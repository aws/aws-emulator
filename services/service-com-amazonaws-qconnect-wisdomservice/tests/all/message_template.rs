// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect message-template slice
//! (`CreateMessageTemplate` + `GetMessageTemplate`).
//!
//! Every assertion goes over the wire, through the same [`Emulator`] harness the tagging
//! suite drives the production router with: only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected, so a response that is not computed from
//! persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a create of a resource nested in a knowledge base persists every member the request
//!   carried and answers out of the STORED record, so a later read — by id, by ARN, and
//!   with or without the `$LATEST` qualifier — reports the same values; the table spans
//!   every channel subtype the model declares, every language code the member documents,
//!   both published bounds of `name`, each optional member the create accepts and a create
//!   that carries none of them;
//! * `messageTemplateContentSha256` is the checksum of the four things the model says it
//!   is calculated over — content, language, `defaultAttributes` and attachments — so two
//!   templates built from the same four have the same checksum, a change to any of them
//!   changes it, and a change to anything else does not;
//! * `attributeTypes` reports the categories of attribute the template holds, derived from
//!   the stored record, and is always reported — the empty list for a template that holds
//!   none;
//! * `channel` is the Amazon Connect contact channel the template's `channelSubtype` is a
//!   subtype of, and `language` is the one the create named or the endpoint's default when
//!   it named none;
//! * a message template's `name` is unique INSIDE one knowledge base: a second create
//!   claiming it is `ConflictException` (HTTP 409) and leaves the first record untouched,
//!   while another knowledge base may hold the same name;
//! * content given under a union member that applies to another channel subtype, and a
//!   `sourceConfiguration` on a subtype that has none, are refused with the modelled
//!   `ValidationException` (HTTP 400) and persist nothing;
//! * a replayed `clientToken` answers with the template that token already created rather
//!   than minting a second one, while the same token carrying a different request is the
//!   modelled idempotency conflict;
//! * a label this knowledge base holds no template under — a bare id, an ARN of this
//!   account, of another account or of another partition, or another knowledge base's
//!   template — is refused by the READ with the modelled `UnauthorizedException` (HTTP
//!   401), which is the answer a read of a template the account does not hold earns,
//!   while a create under a knowledge base this account does not hold is
//!   `ResourceNotFoundException` (HTTP 404), as is every version qualifier that names a
//!   published version while none has been published;
//! * deleting a knowledge base forgets the message templates held inside it;
//! * the tags a create persisted on a template are what `ListTagsForResource` reports for
//!   its ARN;
//! * one knowledge base holds at most the store's message-template ceiling, past which a
//!   create is the modelled `ServiceQuotaExceededException` (HTTP 402);
//! * `CreateMessageTemplateAttachment` uploads one attachment file to the template's
//!   WORKING COPY and the url it reports serves that file — the rules, and the tables they
//!   are asserted over, are stated where that section begins below.
//!
//! This model serves the shared `wisdom` endpoint identity (see `src/service.rs`), which is
//! what every request below is signed for.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::state::{
    DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES, MAX_ATTACHMENTS_PER_MESSAGE_TEMPLATE,
    MAX_MESSAGE_TEMPLATES_PER_KNOWLEDGE_BASE, MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES,
};

use super::message_template_qualifier::MESSAGE_QUALIFIER_NOT_ACCEPTED;
use super::resource_tags::{label, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION};

/// Every value the modelled `ChannelSubtype` enumeration declares
/// (`smithy_query(query="shape",shape="ChannelSubtype")`), paired with the
/// `MessageTemplateContentProvider` member the model documents as the content that
/// applies to it (`smithy_query(query="shape",shape="MessageTemplateContentProvider")`).
/// The catalog is written out here rather than imported, so the table spans the whole
/// value set and not the one subtype a single recording used.
pub(crate) const CHANNEL_SUBTYPES: [(&str, &str); 4] = [
    ("EMAIL", "email"),
    ("SMS", "sms"),
    ("WHATSAPP", "whatsApp"),
    ("PUSH", "push"),
];

/// Every language code the model documents for `language`
/// (`smithy_query(query="doc",shape="CreateMessageTemplateRequest",member="language")`:
/// "The supported language codes include de_DE, en_US, es_ES, fr_FR, id_ID, it_IT, ja_JP,
/// ko_KR, pt_BR, zh_CN, zh_TW").
const LANGUAGES: [&str; 11] = [
    "de_DE", "en_US", "es_ES", "fr_FR", "id_ID", "it_IT", "ja_JP", "ko_KR", "pt_BR", "zh_CN",
    "zh_TW",
];

/// The language code a template created without one reports: `language` is optional on the
/// create while every read reports the template's effective language, so a create that names
/// none is written in the endpoint's default language. Written out here rather than imported,
/// so the assertion pins the answer a client sees.
pub(crate) const DEFAULT_LANGUAGE: &str = "en_US";

/// The Amazon Connect contact channel one `ChannelSubtype` is a subtype of, which is what
/// `channel` reports: an email template is sent on the EMAIL channel, and the messaging
/// subtypes (SMS, WhatsApp Business, mobile push) are subtypes of the CHAT channel — the
/// channel a quick response's `channels` member documents for this same endpoint ("The
/// supported contact channel types include Chat").
pub(crate) fn expected_channel(channel_subtype: &str) -> &'static str {
    match channel_subtype {
        "EMAIL" => "EMAIL",
        _ => "CHAT",
    }
}

/// The published bounds of the members this suite varies in length
/// (`smithy_query(query="operation",operation="CreateMessageTemplate")`: `name` is `len
/// 1..255`, `description` `len 1..255`) and of the email header list
/// (`smithy_query(query="shape",shape="EmailMessageTemplateContent")`: `headers` `len
/// 0..15`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 255;
const DESCRIPTION_MAX: usize = 255;
const EMAIL_HEADERS_MAX: usize = 15;

/// Every value the modelled `MessageTemplateAttributeType` enumeration declares
/// (`smithy_query(query="shape",shape="MessageTemplateAttributeType")`), in the order the
/// model declares them.
const ATTRIBUTE_TYPES: [&str; 4] = ["SYSTEM", "AGENT", "CUSTOMER_PROFILE", "CUSTOM"];

/// The version qualifiers the operation's own documentation names
/// (`smithy_query(query="doc",shape="GetMessageTemplate")`: "either an actual version
/// number or an Amazon Q Connect managed qualifier $ACTIVE_VERSION | $LATEST"). Written
/// out here rather than imported from the implementation.
const QUALIFIER_LATEST: &str = "$LATEST";
const QUALIFIER_ACTIVE: &str = "$ACTIVE_VERSION";

/// The fixed sentences this endpoint answers a resource it does not hold with, written out
/// here rather than imported from the implementation: the one naming the message template,
/// and the one naming its parent knowledge base. Each travels with the label the request
/// used in `ResourceNotFoundException`'s own `resourceName` member.
const MESSAGE_MESSAGE_TEMPLATE_NOT_FOUND: &str = "MessageTemplate does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The fixed sentence, status and error code a READ of a message template this knowledge
/// base does not hold is refused with — the modelled `UnauthorizedException`
/// (`@httpError(401)`), whose only member is `message`. Written out here rather than
/// imported from the implementation.
pub(crate) const MESSAGE_UNAUTHORIZED: &str = "Unauthorized";
pub(crate) const UNAUTHORIZED_STATUS: u16 = 401;
pub(crate) const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The sentence a create under a name a live template of the same knowledge base already
/// holds is refused with, and the one a replayed `clientToken` carrying a different
/// request is refused with.
const MESSAGE_NAME_IN_USE: &str = "Name is already in use";
const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// The sentence a tag set larger than the resource ceiling is refused with.
const MESSAGE_TOO_MANY_TAGS: &str = "The resultant tag set must not have more than 50 user tags.";

/// The ARN `lastModifiedBy` carries: "The Amazon Resource Name (ARN) of the user who last
/// updated the message template data" (`smithy_query(query="shape",shape="MessageTemplateData")`
/// — a `GenericArn`). The emulator publishes ONE long-term access key, and on AWS a
/// long-term key belongs to an IAM user of the account. Written out rather than taken from
/// the implementation's helper, so the assertion pins the value.
fn caller_principal_arn() -> String {
    format!(
        "arn:aws:iam::{account}:user/emulator",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The message-template ARN this service mints:
/// `arn:aws:wisdom:<region>:<account>:message-template/<knowledgeBaseId>/<messageTemplateId>`.
/// Written out here rather than taken from the handler's helper, so the test pins the
/// format.
fn expected_message_template_arn(knowledge_base_id: &str, message_template_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:message-template/{knowledge_base_id}/{message_template_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// Whether `value` is a lowercase hyphenated UUID, the form the model's `Uuid` shape
/// requires (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

/// Whether `value` has the form the modelled `MessageTemplateContentSha256` admits
/// (`len 1..64`, `^[a-zA-Z0-9]+$`) for a SHA-256 digest: 64 lowercase hex digits.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The message-template surface of the shared harness. An inherent impl in this module:
/// the methods are this suite's own, reached only from here.
impl Emulator {
    /// A knowledge base of the type that holds message templates
    /// (`smithy_query(query="shape",shape="KnowledgeBaseType")` declares
    /// `MESSAGE_TEMPLATES` beside `EXTERNAL`, `CUSTOM`, `QUICK_RESPONSES` and `MANAGED`),
    /// answering with the id its create minted.
    async fn create_knowledge_base_named(&self, name: &str) -> String {
        let reply = self
            .send(
                "POST",
                "/knowledgeBases",
                Some(json!({ "name": name, "knowledgeBaseType": "MESSAGE_TEMPLATES" })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: the knowledge base this suite nests under must be created, got \
             HTTP {} with body {}",
            reply.status, reply.body
        );
        let id = reply.string("/knowledgeBase/knowledgeBaseId");
        assert!(
            is_modelled_uuid(&id),
            "case {name:?}: expected a modelled Uuid knowledgeBaseId, got {id:?}"
        );
        id
    }

    async fn create_message_template(&self, knowledge_base: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/knowledgeBases/{}/messageTemplates", label(knowledge_base)),
            Some(body),
        )
        .await
    }

    async fn get_message_template(&self, knowledge_base: &str, message_template: &str) -> Reply {
        self.send(
            "GET",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}",
                label(knowledge_base),
                label(message_template)
            ),
            None,
        )
        .await
    }
}

/// One row of the create/read table: what varies between message templates.
#[derive(Debug, Clone)]
struct TemplateCase {
    label: &'static str,
    name: String,
    channel_subtype: &'static str,
    /// `None` exercises a create that carries no `content` at all, which the model allows.
    content: Option<Value>,
    description: Option<String>,
    language: Option<&'static str>,
    source_configuration: Option<Value>,
    default_attributes: Option<Value>,
    /// The `attributeTypes` the stored template must report, in the model's own order.
    attribute_types: Vec<&'static str>,
    grouping_configuration: Option<Value>,
    tags: BTreeMap<String, String>,
}

impl TemplateCase {
    fn new(label: &'static str, name: &str, channel_subtype: &'static str) -> Self {
        Self {
            label,
            name: name.to_string(),
            channel_subtype,
            content: Some(content_for(channel_subtype)),
            description: None,
            language: None,
            source_configuration: None,
            default_attributes: None,
            attribute_types: Vec::new(),
            grouping_configuration: None,
            tags: BTreeMap::new(),
        }
    }

    /// The request body a client sends for this row: every member it names and no other,
    /// so a row that omits one exercises a create that omits it.
    fn body(&self) -> Value {
        let mut body = json!({ "name": self.name, "channelSubtype": self.channel_subtype });
        if let Some(content) = &self.content {
            body["content"] = content.clone();
        }
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if let Some(language) = self.language {
            body["language"] = json!(language);
        }
        if let Some(source_configuration) = &self.source_configuration {
            body["sourceConfiguration"] = source_configuration.clone();
        }
        if let Some(default_attributes) = &self.default_attributes {
            body["defaultAttributes"] = default_attributes.clone();
        }
        if let Some(grouping_configuration) = &self.grouping_configuration {
            body["groupingConfiguration"] = grouping_configuration.clone();
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        body
    }
}

/// The content union member that applies to one channel subtype, filled with values the
/// modelled constraints of that member admit.
pub(crate) fn content_for(channel_subtype: &str) -> Value {
    match channel_subtype {
        "EMAIL" => json!({
            "email": {
                "subject": "Your order has shipped",
                "body": { "plainText": { "content": "Hello, your order is on its way." } },
            }
        }),
        "SMS" => json!({
            "sms": { "body": { "plainText": { "content": "Your order is on its way." } } }
        }),
        "WHATSAPP" => json!({ "whatsApp": { "data": "{\"name\":\"order_update\"}" } }),
        "PUSH" => json!({
            "push": {
                "apns": {
                    "title": "Order update",
                    "body": { "content": "Your order is on its way." },
                    "action": "OPEN_APP",
                }
            }
        }),
        other => panic!("no content member is modelled for channel subtype {other:?}"),
    }
}

/// A `sourceConfiguration` the modelled constraints admit
/// (`smithy_query(query="shape",shape="WhatsAppMessageTemplateSourceConfiguration")`:
/// `businessAccountId` matches `(^waba-[0-9a-zA-Z]+$)|(^arn:.*:waba/[0-9a-zA-Z]+$)` and
/// `templateId` matches `^[0-9]+$`).
fn whatsapp_source_configuration() -> Value {
    json!({ "whatsApp": { "businessAccountId": "waba-1a2b3c", "templateId": "1234567890" } })
}

/// Every table row this suite creates and reads back.
fn template_cases() -> Vec<TemplateCase> {
    let mut cases = Vec::new();

    // One row per channel subtype, carrying only what the create requires plus the content
    // member that applies to that subtype.
    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let mut case = TemplateCase::new(
            match member {
                "email" => "email-minimal",
                "sms" => "sms-minimal",
                "whatsApp" => "whatsapp-minimal",
                _ => "push-minimal",
            },
            &format!("minimal-{member}"),
            channel_subtype,
        );
        if channel_subtype == "WHATSAPP" {
            case.source_configuration = Some(whatsapp_source_configuration());
        }
        cases.push(case);
    }

    // An EMAIL row carrying EVERY optional member: both body formats, the maximum number
    // of email headers, `description` at its published maximum, a language, all four
    // attribute categories, a grouping configuration, tags, and `name` at its published
    // maximum.
    let mut full = TemplateCase::new("email-every-member", &"n".repeat(NAME_MAX), "EMAIL");
    full.content = Some(json!({
        "email": {
            "subject": "Order {{Customer.FirstName}}",
            "body": {
                "plainText": { "content": "Hello {{Customer.FirstName}}." },
                "html": { "content": "<p>Hello {{Customer.FirstName}}.</p>" },
            },
            "headers": (0..EMAIL_HEADERS_MAX)
                .map(|index| json!({ "name": format!("X-Emulator-{index}"), "value": "set" }))
                .collect::<Vec<Value>>(),
        }
    }));
    full.description = Some("d".repeat(DESCRIPTION_MAX));
    full.language = Some("en_US");
    full.default_attributes = Some(json!({
        "systemAttributes": { "name": "Contact", "customerEndpoint": { "address": "+15550100" } },
        "agentAttributes": { "firstName": "Ada", "lastName": "Lovelace" },
        "customerProfileAttributes": { "firstName": "Grace", "custom": { "tier": "gold" } },
        "customAttributes": { "orderId": "A-1" },
    }));
    full.attribute_types = ATTRIBUTE_TYPES.to_vec();
    full.grouping_configuration =
        Some(json!({ "criteria": "RoutingProfileArn", "values": ["arn:aws:connect:::rp/one"] }));
    full.tags = BTreeMap::from([
        ("env".to_string(), "test".to_string()),
        ("owner".to_string(), "emulator".to_string()),
    ]);
    cases.push(full);

    // `name` at its published MINIMUM, and a create that names no content at all — which
    // the model allows, so the stored template reports none.
    let mut shortest = TemplateCase::new("sms-shortest-name", &"n".repeat(NAME_MIN), "SMS");
    shortest.language = Some("de_DE");
    cases.push(shortest);

    let mut contentless = TemplateCase::new("email-no-content", "no-content", "EMAIL");
    contentless.content = None;
    cases.push(contentless);

    // One row per attribute category on its own, so each category's contribution to
    // `attributeTypes` is asserted rather than only the all-four case.
    let per_category: [(&'static str, &'static str, Value, &'static str); 4] = [
        (
            "attributes-system-only",
            "SMS",
            json!({ "systemAttributes": { "name": "Contact" } }),
            "SYSTEM",
        ),
        (
            "attributes-agent-only",
            "EMAIL",
            json!({ "agentAttributes": { "firstName": "Ada" } }),
            "AGENT",
        ),
        (
            "attributes-customer-profile-only",
            "PUSH",
            json!({ "customerProfileAttributes": { "lastName": "Hopper" } }),
            "CUSTOMER_PROFILE",
        ),
        (
            "attributes-custom-only",
            "SMS",
            json!({ "customAttributes": { "orderId": "A-2" } }),
            "CUSTOM",
        ),
    ];
    for (label, channel_subtype, attributes, expected) in per_category {
        let mut case = TemplateCase::new(label, label, channel_subtype);
        case.default_attributes = Some(attributes);
        case.attribute_types = vec![expected];
        cases.push(case);
    }

    // An EMPTY `customAttributes` map declares no attribute, so it contributes no type.
    let mut empty_custom = TemplateCase::new("attributes-empty-custom", "empty-custom", "SMS");
    empty_custom.default_attributes = Some(json!({ "customAttributes": {} }));
    cases.push(empty_custom);

    cases
}

/// Assert that `data` (a `MessageTemplateData` or an `ExtendedMessageTemplateData`, at
/// `pointer`) reports exactly what `case` asked for under `knowledge_base_id`.
fn assert_reports_case(
    reply: &Reply,
    pointer: &str,
    case: &TemplateCase,
    knowledge_base_id: &str,
    expected_arn: &str,
    reader: &str,
) {
    let case_label = case.label;
    assert_eq!(
        reply.status, 200,
        "case {case_label:?} read by {reader}: expected HTTP 200, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let field = |member: &str| reply.field(&format!("{pointer}/{member}")).cloned();
    let string = |member: &str| reply.string(&format!("{pointer}/{member}"));

    assert_eq!(
        string("messageTemplateArn"),
        expected_arn,
        "case {case_label:?} read by {reader}: expected messageTemplateArn {expected_arn:?}, got \
         {:?}",
        string("messageTemplateArn")
    );
    assert_eq!(
        string("knowledgeBaseId"),
        knowledge_base_id,
        "case {case_label:?} read by {reader}: expected knowledgeBaseId {knowledge_base_id:?}, \
         got {:?}",
        string("knowledgeBaseId")
    );
    assert_eq!(
        string("name"),
        case.name,
        "case {case_label:?} read by {reader}: expected name of length {}, got {:?}",
        case.name.len(),
        string("name").len()
    );
    assert_eq!(
        string("channelSubtype"),
        case.channel_subtype,
        "case {case_label:?} read by {reader}: expected channelSubtype {:?}, got {:?}",
        case.channel_subtype,
        string("channelSubtype")
    );
    assert_eq!(
        field("content"),
        case.content.clone(),
        "case {case_label:?} read by {reader}: expected content {:?}, got {:?}",
        case.content,
        field("content")
    );
    assert_eq!(
        field("description"),
        case.description.as_ref().map(|value| json!(value)),
        "case {case_label:?} read by {reader}: expected description of length {:?}, got {:?}",
        case.description.as_ref().map(String::len),
        field("description")
    );
    assert_eq!(
        field("language"),
        Some(json!(case.language.unwrap_or(DEFAULT_LANGUAGE))),
        "case {case_label:?} read by {reader}: expected language {:?}, got {:?}",
        case.language.unwrap_or(DEFAULT_LANGUAGE),
        field("language")
    );
    assert_eq!(
        field("defaultAttributes"),
        case.default_attributes.clone(),
        "case {case_label:?} read by {reader}: expected defaultAttributes {:?}, got {:?}",
        case.default_attributes,
        field("defaultAttributes")
    );
    assert_eq!(
        field("groupingConfiguration"),
        case.grouping_configuration.clone(),
        "case {case_label:?} read by {reader}: expected groupingConfiguration {:?}, got {:?}",
        case.grouping_configuration,
        field("groupingConfiguration")
    );
    // The list is ALWAYS reported, as the empty list for a template that holds no attribute
    // of any category: a client that iterates it must be able to tell "none" from "not
    // returned".
    let expected_attribute_types = json!(case.attribute_types);
    assert_eq!(
        field("attributeTypes"),
        Some(expected_attribute_types.clone()),
        "case {case_label:?} read by {reader}: expected attributeTypes {expected_attribute_types:?}, \
         got {:?}",
        field("attributeTypes")
    );
    // The source configuration is reported as the modelled SUMMARY, carrying the two ids
    // the create gave and nothing this emulator does not hold.
    let expected_summary = case.source_configuration.as_ref().map(|_| {
        json!({ "whatsApp": { "businessAccountId": "waba-1a2b3c", "templateId": "1234567890" } })
    });
    assert_eq!(
        field("sourceConfigurationSummary"),
        expected_summary,
        "case {case_label:?} read by {reader}: expected sourceConfigurationSummary \
         {expected_summary:?}, got {:?}",
        field("sourceConfigurationSummary")
    );
    assert_eq!(
        field("tags"),
        Some(json!(case.tags)),
        "case {case_label:?} read by {reader}: expected tags {:?}, got {:?}",
        case.tags,
        field("tags")
    );
    assert_eq!(
        string("lastModifiedBy"),
        caller_principal_arn(),
        "case {case_label:?} read by {reader}: expected lastModifiedBy {:?}, got {:?}",
        caller_principal_arn(),
        string("lastModifiedBy")
    );
    assert_eq!(
        string("createdTime"),
        string("lastModifiedTime"),
        "case {case_label:?} read by {reader}: nothing has modified the record, so \
         lastModifiedTime must equal createdTime, got {:?} and {:?}",
        string("createdTime"),
        string("lastModifiedTime")
    );
    let checksum = string("messageTemplateContentSha256");
    assert!(
        is_sha256_hex(&checksum),
        "case {case_label:?} read by {reader}: expected a 64-digit hex content checksum, got \
         {checksum:?}"
    );
    // `channel` is the Amazon Connect contact channel the template's subtype is a subtype
    // of, which no request member carries and every projection derives from the subtype.
    let expected_channel = expected_channel(case.channel_subtype);
    assert_eq!(
        field("channel"),
        Some(json!(expected_channel)),
        "case {case_label:?} read by {reader}: expected channel {expected_channel:?} for channel \
         subtype {:?}, got {:?}",
        case.channel_subtype,
        field("channel")
    );
}

/// The rule: a create persists every member the request carried and answers out of the
/// stored record, so every later read — by id, by ARN, and with or without the `$LATEST`
/// qualifier — reports the same values.
#[tokio::test]
async fn create_persists_every_member_and_every_read_reports_it() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("templates").await;

    for case in template_cases() {
        let created = emulator
            .create_message_template(&knowledge_base_id, case.body())
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from the create, got HTTP {} with body {}",
            case.label, created.status, created.body
        );
        let message_template_id = created.string("/messageTemplate/messageTemplateId");
        assert!(
            is_modelled_uuid(&message_template_id),
            "case {:?}: expected a modelled Uuid messageTemplateId, got {message_template_id:?}",
            case.label
        );
        let bare_arn = expected_message_template_arn(&knowledge_base_id, &message_template_id);
        assert_eq!(
            created.string("/messageTemplate/knowledgeBaseArn"),
            wisdom_arn("aws", REGION, engine::EMULATOR_ACCOUNT_ID, &format!("knowledge-base/{knowledge_base_id}")),
            "case {:?}: the template must report its knowledge base's own ARN, got {:?}",
            case.label,
            created.string("/messageTemplate/knowledgeBaseArn")
        );

        // The create addressed no version, so it reports the bare ARN and no version
        // members at all.
        assert_reports_case(
            &created,
            "/messageTemplate",
            &case,
            &knowledge_base_id,
            &bare_arn,
            "the create itself",
        );
        for absent in ["versionNumber", "isActive", "attachments"] {
            assert_eq!(
                created.field(&format!("/messageTemplate/{absent}")),
                None,
                "case {:?}: MessageTemplateData declares no {absent}, got {:?}",
                case.label,
                created.field(&format!("/messageTemplate/{absent}"))
            );
        }

        // Every label form that names this template, with and without the qualifier that
        // addresses the working copy.
        let readers: [(String, String); 4] = [
            (message_template_id.clone(), format!("{bare_arn}:{QUALIFIER_LATEST}")),
            (bare_arn.clone(), format!("{bare_arn}:{QUALIFIER_LATEST}")),
            (
                format!("{message_template_id}:{QUALIFIER_LATEST}"),
                format!("{bare_arn}:{QUALIFIER_LATEST}"),
            ),
            (
                format!("{bare_arn}:{QUALIFIER_LATEST}"),
                format!("{bare_arn}:{QUALIFIER_LATEST}"),
            ),
        ];
        for (template_label, expected_arn) in readers {
            let read = emulator
                .get_message_template(&knowledge_base_id, &template_label)
                .await;
            assert_reports_case(
                &read,
                "/messageTemplate",
                &case,
                &knowledge_base_id,
                &expected_arn,
                &format!("the label {template_label:?}"),
            );
            assert_eq!(
                read.string("/messageTemplate/messageTemplateContentSha256"),
                created.string("/messageTemplate/messageTemplateContentSha256"),
                "case {:?} read by {template_label:?}: the checksum is derived from the stored \
                 record, so a read must report the one the create reported, got {:?} and {:?}",
                case.label,
                read.string("/messageTemplate/messageTemplateContentSha256"),
                created.string("/messageTemplate/messageTemplateContentSha256")
            );
            // The working copy is not a version, so neither version member is claimed.
            for absent in ["versionNumber", "isActive", "attachments"] {
                assert_eq!(
                    read.field(&format!("/messageTemplate/{absent}")),
                    None,
                    "case {:?} read by {template_label:?}: the working copy is no version and \
                     holds no attachment, so {absent} must be absent, got {:?}",
                    case.label,
                    read.field(&format!("/messageTemplate/{absent}"))
                );
            }
        }

        // The tags the create persisted are the ones the tagging surface reports for the
        // template's own ARN.
        let tags = emulator.list_tags(&bare_arn).await;
        assert_eq!(
            tags.field("/tags").cloned(),
            Some(json!(case.tags)),
            "case {:?}: ListTagsForResource must report the tags the create persisted {:?}, got \
             {:?}",
            case.label,
            case.tags,
            tags.field("/tags")
        );
    }
}

/// The rule: every documented language code round-trips, so the stored template reports the
/// code the create named whichever one it is.
#[tokio::test]
async fn every_documented_language_round_trips() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("languages").await;

    for language in LANGUAGES {
        let created = emulator
            .create_message_template(
                &knowledge_base_id,
                json!({
                    "name": format!("lang-{language}"),
                    "channelSubtype": "SMS",
                    "content": content_for("SMS"),
                    "language": language,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {language:?}: expected HTTP 200 from the create, got HTTP {} with body {}",
            created.status, created.body
        );
        let read = emulator
            .get_message_template(
                &knowledge_base_id,
                &created.string("/messageTemplate/messageTemplateId"),
            )
            .await;
        assert_eq!(
            read.string("/messageTemplate/language"),
            language,
            "case {language:?}: expected the stored language {language:?}, got {:?}",
            read.string("/messageTemplate/language")
        );
    }
}

/// The rule the model states on the member: `messageTemplateContentSha256` is calculated
/// over content, language, `defaultAttributes` and attachments — so two templates built
/// from the same four agree, a change to any of them changes it, and a change to anything
/// else leaves it alone.
#[tokio::test]
async fn content_checksum_follows_the_four_things_it_is_calculated_over() {
    let emulator = Emulator::new();
    let first_base = emulator.create_knowledge_base_named("checksum-one").await;
    let second_base = emulator.create_knowledge_base_named("checksum-two").await;

    let base_body = |name: &str| {
        json!({
            "name": name,
            "channelSubtype": "EMAIL",
            "content": content_for("EMAIL"),
            "language": "en_US",
            "defaultAttributes": { "customAttributes": { "orderId": "A-1" } },
        })
    };
    let checksum = |reply: &Reply| reply.string("/messageTemplate/messageTemplateContentSha256");

    let reference = emulator
        .create_message_template(&first_base, base_body("reference"))
        .await;
    assert_eq!(
        reference.status, 200,
        "case \"reference\": expected HTTP 200, got HTTP {} with body {}",
        reference.status, reference.body
    );
    let reference_checksum = checksum(&reference);

    // Same four parts, a DIFFERENT name, description, grouping configuration, tags and
    // knowledge base: none of those is one of the four, so the checksum is the same.
    let mut elsewhere = base_body("renamed-elsewhere");
    elsewhere["description"] = json!("a different description");
    elsewhere["tags"] = json!({ "env": "other" });
    let same = emulator.create_message_template(&second_base, elsewhere).await;
    assert_eq!(
        checksum(&same),
        reference_checksum,
        "case \"same-four-parts\": the checksum covers content, language, defaultAttributes and \
         attachments only, so it must equal {reference_checksum:?}, got {:?}",
        checksum(&same)
    );

    // One differing part at a time, each of the three the create can carry.
    let mut differing: Vec<(&str, Value)> = Vec::new();
    let mut other_content = base_body("other-content");
    other_content["content"] = json!({
        "email": {
            "subject": "Your order has shipped",
            "body": { "plainText": { "content": "A different body." } },
        }
    });
    differing.push(("content", other_content));
    let mut other_language = base_body("other-language");
    other_language["language"] = json!("fr_FR");
    differing.push(("language", other_language));
    let mut other_attributes = base_body("other-attributes");
    other_attributes["defaultAttributes"] = json!({ "customAttributes": { "orderId": "A-2" } });
    differing.push(("defaultAttributes", other_attributes));
    let mut no_content = base_body("without-content");
    no_content.as_object_mut().expect("object body").remove("content");
    differing.push(("content absent", no_content));

    for (part, body) in differing {
        let reply = emulator.create_message_template(&first_base, body).await;
        assert_eq!(
            reply.status, 200,
            "case {part:?}: expected HTTP 200, got HTTP {} with body {}",
            reply.status, reply.body
        );
        assert_ne!(
            checksum(&reply),
            reference_checksum,
            "case {part:?}: the checksum is calculated over this part, so changing it must change \
             the checksum away from {reference_checksum:?}, got {:?}",
            checksum(&reply)
        );
    }
}

/// The rule: a message template's `name` identifies it INSIDE its knowledge base, so a
/// second create claiming it conflicts and leaves the first untouched, while another
/// knowledge base may hold the same name.
#[tokio::test]
async fn a_name_identifies_one_template_inside_its_knowledge_base() {
    let emulator = Emulator::new();
    let first_base = emulator.create_knowledge_base_named("names-one").await;
    let second_base = emulator.create_knowledge_base_named("names-two").await;

    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let name = format!("shared-{member}");
        let mut body = json!({
            "name": name,
            "channelSubtype": channel_subtype,
            "content": content_for(channel_subtype),
        });
        if channel_subtype == "WHATSAPP" {
            body["sourceConfiguration"] = whatsapp_source_configuration();
        }
        let first = emulator
            .create_message_template(&first_base, body.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: expected HTTP 200 from the first create, got HTTP {} with body {}",
            first.status, first.body
        );
        let first_id = first.string("/messageTemplate/messageTemplateId");

        let again = emulator
            .create_message_template(&first_base, body.clone())
            .await;
        assert_eq!(
            again.status, 409,
            "case {name:?}: a second create under a live name must be HTTP 409, got HTTP {} with \
             body {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type, "ConflictException",
            "case {name:?}: expected error type ConflictException, got {:?}",
            again.error_type
        );
        assert_eq!(
            again.string("/message"),
            MESSAGE_NAME_IN_USE,
            "case {name:?}: expected the message {MESSAGE_NAME_IN_USE:?}, got {:?}",
            again.string("/message")
        );

        // The refusal left the first record exactly where it was.
        let read = emulator.get_message_template(&first_base, &first_id).await;
        assert_eq!(
            read.string("/messageTemplate/name"),
            name,
            "case {name:?}: the refused create must not touch the first record, got name {:?}",
            read.string("/messageTemplate/name")
        );

        // Another knowledge base may hold the same name: the name identifies a template
        // inside ONE knowledge base.
        let other = emulator
            .create_message_template(&second_base, body)
            .await;
        assert_eq!(
            other.status, 200,
            "case {name:?}: another knowledge base may hold the same name, got HTTP {} with body \
             {}",
            other.status, other.body
        );
        assert_ne!(
            other.string("/messageTemplate/messageTemplateId"),
            first_id,
            "case {name:?}: the two templates must be two records, got one id {first_id:?}"
        );
    }
}

/// The rule: content given under a union member that applies to another channel subtype,
/// and a `sourceConfiguration` on a subtype that has none, are refused and persist nothing.
#[tokio::test]
async fn content_and_source_configuration_must_apply_to_the_channel_subtype() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("subtypes").await;

    // Every (subtype, content member) pair the model does NOT pair.
    for (channel_subtype, _) in CHANNEL_SUBTYPES {
        for (other_subtype, other_member) in CHANNEL_SUBTYPES {
            if other_subtype == channel_subtype {
                continue;
            }
            let case = format!("{channel_subtype} with {other_member} content");
            let reply = emulator
                .create_message_template(
                    &knowledge_base_id,
                    json!({
                        "name": format!("mismatch-{channel_subtype}-{other_member}"),
                        "channelSubtype": channel_subtype,
                        "content": content_for(other_subtype),
                    }),
                )
                .await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got HTTP {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "ValidationException",
                "case {case:?}: expected error type ValidationException, got {:?}",
                reply.error_type
            );
            let message = reply.string("/message");
            assert!(
                message.contains(other_member) && message.contains(channel_subtype),
                "case {case:?}: the refusal must name the member it was given and the subtype it \
                 was named for, got {message:?}"
            );
        }
    }

    // `sourceConfiguration` is only for WHATSAPP, so it is refused for every other
    // subtype.
    for (channel_subtype, _) in CHANNEL_SUBTYPES {
        if channel_subtype == "WHATSAPP" {
            continue;
        }
        let reply = emulator
            .create_message_template(
                &knowledge_base_id,
                json!({
                    "name": format!("sourced-{channel_subtype}"),
                    "channelSubtype": channel_subtype,
                    "content": content_for(channel_subtype),
                    "sourceConfiguration": whatsapp_source_configuration(),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {channel_subtype:?} with a sourceConfiguration: expected HTTP 400, got HTTP {} \
             with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ValidationException",
            "case {channel_subtype:?} with a sourceConfiguration: expected error type \
             ValidationException, got {:?}",
            reply.error_type
        );
        assert!(
            reply.string("/message").contains(channel_subtype),
            "case {channel_subtype:?} with a sourceConfiguration: the refusal must name the \
             subtype the request used, got {:?}",
            reply.string("/message")
        );
    }

    // A create this endpoint refused persisted nothing: the names above are free, and a
    // create that pairs the content with its own subtype succeeds under one of them.
    let accepted = emulator
        .create_message_template(
            &knowledge_base_id,
            json!({
                "name": "sourced-EMAIL",
                "channelSubtype": "EMAIL",
                "content": content_for("EMAIL"),
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"name freed by a refusal\": a refused create must persist nothing, got HTTP {} \
         with body {}",
        accepted.status, accepted.body
    );
}

/// The rule: `MessageTemplateData.name` is required, so a create that names none is refused
/// where an absent member is refused rather than producing a template without one.
#[tokio::test]
async fn a_create_that_names_no_name_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("nameless").await;

    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let reply = emulator
            .create_message_template(
                &knowledge_base_id,
                json!({ "channelSubtype": channel_subtype, "content": content_for(channel_subtype) }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {member:?}: expected HTTP 400 for a create naming no name, got HTTP {} with \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ValidationException",
            "case {member:?}: expected error type ValidationException, got {:?}",
            reply.error_type
        );
        let message = reply.string("/message");
        assert!(
            message.contains("'name'") && message.contains("must not be null"),
            "case {member:?}: the refusal must state that the member must not be null, got \
             {message:?}"
        );
    }
}

/// The rule: a replayed `clientToken` answers with the template that token already created,
/// while the same token carrying a different request is the modelled idempotency conflict.
#[tokio::test]
async fn a_replayed_client_token_answers_with_what_it_created() {
    let emulator = Emulator::new();
    let first_base = emulator.create_knowledge_base_named("tokens-one").await;
    let second_base = emulator.create_knowledge_base_named("tokens-two").await;

    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let token = format!("token-{member}");
        let mut body = json!({
            "name": format!("token-{member}"),
            "channelSubtype": channel_subtype,
            "content": content_for(channel_subtype),
            "clientToken": token,
        });
        if channel_subtype == "WHATSAPP" {
            body["sourceConfiguration"] = whatsapp_source_configuration();
        }
        let first = emulator
            .create_message_template(&first_base, body.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected HTTP 200 from the first create, got HTTP {} with body {}",
            first.status, first.body
        );
        let first_id = first.string("/messageTemplate/messageTemplateId");

        let retry = emulator
            .create_message_template(&first_base, body.clone())
            .await;
        assert_eq!(
            retry.status, 200,
            "case {token:?}: a retry carrying the same token and request must succeed, got HTTP \
             {} with body {}",
            retry.status, retry.body
        );
        assert_eq!(
            retry.string("/messageTemplate/messageTemplateId"),
            first_id,
            "case {token:?}: a retry must answer with the template the token already created \
             {first_id:?}, got {:?}",
            retry.string("/messageTemplate/messageTemplateId")
        );

        // The same token carrying a DIFFERENT request is an idempotency conflict.
        let mut changed = body.clone();
        changed["description"] = json!("a different request");
        let conflict = emulator
            .create_message_template(&first_base, changed)
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {token:?}: the same token on a different request must be HTTP 409, got HTTP {} \
             with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.string("/message"),
            MESSAGE_IDEMPOTENCY_CONFLICT,
            "case {token:?}: expected the message {MESSAGE_IDEMPOTENCY_CONFLICT:?}, got {:?}",
            conflict.string("/message")
        );

        // The token is scoped to the knowledge base it created under, so the same token
        // under another parent is another resource.
        let elsewhere = emulator
            .create_message_template(&second_base, body)
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {token:?}: the same token under another knowledge base is another create, got \
             HTTP {} with body {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.string("/messageTemplate/messageTemplateId"),
            first_id,
            "case {token:?}: the token is scoped to its parent, so this must be a second record, \
             got {first_id:?}"
        );
    }

    // A request whose members include MAPS is one request however those maps iterate: a
    // replay of it must be answered with the record the token already created rather than
    // refused as a different request. The rows grow the two maps the attributes shape
    // reaches, so no single-entry map can hide an order-dependent fingerprint.
    for entries in [1usize, 2, 8, 32] {
        let token = format!("mapped-{entries}");
        let custom: BTreeMap<String, String> = (0..entries)
            .map(|index| (format!("custom-{index}"), format!("value-{index}")))
            .collect();
        let profile_custom: BTreeMap<String, String> = (0..entries)
            .map(|index| (format!("profile-{index}"), format!("value-{index}")))
            .collect();
        let body = json!({
            "name": format!("mapped-{entries}"),
            "channelSubtype": "SMS",
            "content": content_for("SMS"),
            "clientToken": token,
            "defaultAttributes": {
                "customAttributes": custom,
                "customerProfileAttributes": { "firstName": "Ada", "custom": profile_custom },
            },
            "tags": { "env": "test", "owner": "emulator" },
        });
        let first = emulator
            .create_message_template(&first_base, body.clone())
            .await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: expected HTTP 200 from the first create, got HTTP {} with body {}",
            first.status, first.body
        );
        let first_id = first.string("/messageTemplate/messageTemplateId");
        // Several replays: each one builds fresh maps, so an order-dependent fingerprint
        // would refuse one of them.
        for attempt in 0..4 {
            let retry = emulator
                .create_message_template(&first_base, body.clone())
                .await;
            assert_eq!(
                retry.string("/messageTemplate/messageTemplateId"),
                first_id,
                "case {token:?} replay {attempt}: a replay of the identical request must answer \
                 with {first_id:?}, got HTTP {} with body {}",
                retry.status,
                retry.body
            );
        }
        // And a request that genuinely differs in one map entry is still a different
        // request.
        let mut changed = body.clone();
        changed["defaultAttributes"]["customAttributes"]["custom-0"] = json!("changed");
        let conflict = emulator
            .create_message_template(&first_base, changed)
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {token:?}: one changed map entry is a different request, got HTTP {} with body \
             {}",
            conflict.status, conflict.body
        );
    }
}

/// The rule: a READ whose label pair names no message template of this knowledge base is
/// refused with the modelled `UnauthorizedException` (HTTP 401) and the service's fixed
/// sentence for the condition, whatever form the label takes — while a CREATE under a
/// knowledge base this account does not hold reports that knowledge base as the resource
/// that is missing.
#[tokio::test]
async fn a_label_that_names_no_template_of_this_knowledge_base_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("missing-one").await;
    let other_base_id = emulator.create_knowledge_base_named("missing-two").await;

    let created = emulator
        .create_message_template(
            &other_base_id,
            json!({
                "name": "held-elsewhere",
                "channelSubtype": "SMS",
                "content": content_for("SMS"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"held-elsewhere\": expected HTTP 200, got HTTP {} with body {}",
        created.status, created.body
    );
    let elsewhere_id = created.string("/messageTemplate/messageTemplateId");
    let elsewhere_arn = created.string("/messageTemplate/messageTemplateArn");

    // Every label form that names no template of THIS knowledge base, which holds none at
    // all.
    let labels: [(&str, String); 3] = [
        ("a bare id nothing holds", ABSENT_UUID.to_string()),
        (
            "an ARN of this account nothing holds",
            expected_message_template_arn(&knowledge_base_id, ABSENT_UUID),
        ),
        ("another knowledge base's template", elsewhere_id.clone()),
    ];
    // And every ARN that names the HELD template's own knowledge base and id while
    // differing from the ARN this emulator minted in exactly one identity field this
    // service's label resolution reads: the partition and the account. Such an ARN names a
    // resource of ANOTHER partition or account, which this one does not hold, so it resolves
    // to nothing even though its path is the right one. Each is read under the knowledge
    // base that DOES hold the template, so nothing but that one field can make it miss. (An
    // ARN of another SERVICE is refused earlier still, by the label's own modelled
    // `@pattern`, which admits only this service's namespace.)
    let template_path = format!("message-template/{other_base_id}/{elsewhere_id}");
    let foreign: [(&str, String); 2] = [
        (
            "an ARN of another partition",
            wisdom_arn(
                "aws-cn",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &template_path,
            ),
        ),
        (
            "an ARN of another account",
            wisdom_arn("aws", REGION, OTHER_ACCOUNT, &template_path),
        ),
    ];
    let rows = labels
        .into_iter()
        .map(|(case, label)| (case, label, knowledge_base_id.clone()))
        .chain(
            foreign
                .into_iter()
                .map(|(case, label)| (case, label, other_base_id.clone())),
        );
    for (case, template_label, parent) in rows {
        let reply = emulator.get_message_template(&parent, &template_label).await;
        assert_eq!(
            reply.status, UNAUTHORIZED_STATUS,
            "case {case:?}: expected HTTP {UNAUTHORIZED_STATUS}, got HTTP {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, UNAUTHORIZED_CODE,
            "case {case:?}: expected error type {UNAUTHORIZED_CODE}, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_UNAUTHORIZED,
            "case {case:?}: expected the message {MESSAGE_UNAUTHORIZED:?}, got {:?}",
            reply.string("/message")
        );
        assert!(
            !reply.body.to_string().contains(template_label.as_str()),
            "case {case:?}: the refusal must report nothing the request carried, got {}",
            reply.body
        );
    }

    // And the template's own ARN, under its own knowledge base, still resolves — so the
    // rows above miss because the label names nothing here, not because ARNs do not work.
    let read = emulator
        .get_message_template(&other_base_id, &elsewhere_arn)
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the template's own ARN\": expected HTTP 200, got HTTP {} with body {}",
        read.status, read.body
    );

    // A knowledge base this account does not hold is reported as the resource that is
    // missing, on both the create and the read.
    let absent_create = emulator
        .create_message_template(
            ABSENT_UUID,
            json!({ "name": "orphan", "channelSubtype": "SMS", "content": content_for("SMS") }),
        )
        .await;
    assert_eq!(
        absent_create.status, 404,
        "case \"create under an absent knowledge base\": expected HTTP 404, got HTTP {} with body \
         {}",
        absent_create.status, absent_create.body
    );
    assert_eq!(
        absent_create.string("/message"),
        MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        "case \"create under an absent knowledge base\": expected the message \
         {MESSAGE_KNOWLEDGE_BASE_NOT_FOUND:?}, got {:?}",
        absent_create.string("/message")
    );
    let absent_read = emulator
        .get_message_template(ABSENT_UUID, &elsewhere_id)
        .await;
    assert_eq!(
        absent_read.string("/message"),
        MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        "case \"read under an absent knowledge base\": expected the message \
         {MESSAGE_KNOWLEDGE_BASE_NOT_FOUND:?}, got {:?}",
        absent_read.string("/message")
    );
}

/// The rule the operation documents: the label's qualifier is a version number or one of
/// the two managed qualifiers. `$LATEST` (and an absent qualifier) addresses the working
/// copy; every qualifier that names a PUBLISHED version names nothing while none has been
/// published; a qualifier that is neither is refused.
#[tokio::test]
async fn a_version_qualifier_addresses_the_working_copy_or_a_published_version() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("qualifiers").await;
    let created = emulator
        .create_message_template(
            &knowledge_base_id,
            json!({
                "name": "qualified",
                "channelSubtype": "SMS",
                "content": content_for("SMS"),
            }),
        )
        .await;
    let message_template_id = created.string("/messageTemplate/messageTemplateId");

    // Published versions: a number, both published-version bounds the modelled `Version`
    // range admits at its low end, and the managed qualifier that names the activated one.
    // None has been published, so each names nothing.
    for qualifier in ["1", "2", "99", QUALIFIER_ACTIVE] {
        let template_label = format!("{message_template_id}:{qualifier}");
        let reply = emulator
            .get_message_template(&knowledge_base_id, &template_label)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {qualifier:?}: no version has been published, so the qualifier names nothing; \
             expected HTTP 404, got HTTP {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {qualifier:?}: expected error type ResourceNotFoundException, got {:?}",
            reply.error_type
        );
    }

    // A qualifier the model's pattern admits but the operation does not name.
    for qualifier in ["ACTIVE", "0", "$PREVIOUS"] {
        let template_label = format!("{message_template_id}:{qualifier}");
        let reply = emulator
            .get_message_template(&knowledge_base_id, &template_label)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {qualifier:?}: expected HTTP 400 for a qualifier that names neither the working \
             copy nor a version, got HTTP {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ValidationException",
            "case {qualifier:?}: expected error type ValidationException, got {:?}",
            reply.error_type
        );
        assert!(
            reply.string("/message").contains(qualifier),
            "case {qualifier:?}: the refusal must name the qualifier the request used, got {:?}",
            reply.string("/message")
        );
    }
}

/// The rule: a template lives INSIDE its knowledge base, so deleting the knowledge base
/// forgets the templates held in it and frees the names they held.
#[tokio::test]
async fn deleting_a_knowledge_base_forgets_its_message_templates() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("doomed").await;

    let mut ids = Vec::new();
    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let mut body = json!({
            "name": format!("doomed-{member}"),
            "channelSubtype": channel_subtype,
            "content": content_for(channel_subtype),
            "clientToken": format!("doomed-{member}"),
        });
        if channel_subtype == "WHATSAPP" {
            body["sourceConfiguration"] = whatsapp_source_configuration();
        }
        let created = emulator
            .create_message_template(&knowledge_base_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {member:?}: expected HTTP 200, got HTTP {} with body {}",
            created.status, created.body
        );
        ids.push((
            member,
            created.string("/messageTemplate/messageTemplateId"),
            created.string("/messageTemplate/messageTemplateArn"),
        ));
    }

    let deleted = emulator.delete_knowledge_base(&knowledge_base_id).await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete the knowledge base\": expected HTTP 204, got HTTP {} with body {}",
        deleted.status, deleted.body
    );

    for (member, message_template_id, message_template_arn) in &ids {
        let read = emulator
            .get_message_template(&knowledge_base_id, message_template_id)
            .await;
        assert_eq!(
            read.status, 404,
            "case {member:?}: the knowledge base is gone, so its template is too; expected HTTP \
             404, got HTTP {} with body {}",
            read.status, read.body
        );
        let tags = emulator.list_tags(message_template_arn).await;
        assert_eq!(
            tags.status, 404,
            "case {member:?}: the tagging surface must not hold a record the delete reclaimed, \
             got HTTP {} with body {}",
            tags.status, tags.body
        );
    }

    // A fresh knowledge base may take the same names and the same tokens again: both
    // identified a LIVE template of a knowledge base that is gone.
    let reborn = emulator.create_knowledge_base_named("reborn").await;
    for (channel_subtype, member) in CHANNEL_SUBTYPES {
        let mut body = json!({
            "name": format!("doomed-{member}"),
            "channelSubtype": channel_subtype,
            "content": content_for(channel_subtype),
            "clientToken": format!("doomed-{member}"),
        });
        if channel_subtype == "WHATSAPP" {
            body["sourceConfiguration"] = whatsapp_source_configuration();
        }
        let created = emulator.create_message_template(&reborn, body).await;
        assert_eq!(
            created.status, 200,
            "case {member:?}: the name a reclaimed template held must be free, got HTTP {} with \
             body {}",
            created.status, created.body
        );
    }
}

/// The rule: the `tags` map the model puts no bound on is bounded by the resource ceiling
/// AWS's tagging restrictions state, and a create past it persists nothing.
#[tokio::test]
async fn a_create_past_the_tag_ceiling_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("tag-ceiling").await;

    let tags: BTreeMap<String, String> = (0..51)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let reply = emulator
        .create_message_template(
            &knowledge_base_id,
            json!({
                "name": "over-tagged",
                "channelSubtype": "SMS",
                "content": content_for("SMS"),
                "tags": tags,
            }),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "case \"51 tags\": expected HTTP 400, got HTTP {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("/message"),
        MESSAGE_TOO_MANY_TAGS,
        "case \"51 tags\": expected the message {MESSAGE_TOO_MANY_TAGS:?}, got {:?}",
        reply.string("/message")
    );

    // The ceiling itself is accepted, and the refusal above persisted nothing.
    let at_ceiling: BTreeMap<String, String> = (0..50)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let accepted = emulator
        .create_message_template(
            &knowledge_base_id,
            json!({
                "name": "over-tagged",
                "channelSubtype": "SMS",
                "content": content_for("SMS"),
                "tags": at_ceiling,
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"50 tags\": the ceiling itself is accepted, got HTTP {} with body {}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted
            .field("/messageTemplate/tags")
            .and_then(Value::as_object)
            .map(serde_json::Map::len),
        Some(50),
        "case \"50 tags\": expected all 50 tags on the stored record, got {:?}",
        accepted.field("/messageTemplate/tags")
    );
}

/// The rule: one knowledge base holds at most the store's message-template ceiling, past
/// which a create is the modelled `ServiceQuotaExceededException` (HTTP 402).
#[tokio::test]
async fn a_create_past_the_message_template_ceiling_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("quota").await;

    for index in 0..MAX_MESSAGE_TEMPLATES_PER_KNOWLEDGE_BASE {
        let reply = emulator
            .create_message_template(
                &knowledge_base_id,
                json!({
                    "name": format!("bulk-{index}"),
                    "channelSubtype": "SMS",
                    "content": content_for("SMS"),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index}: a create inside the ceiling must succeed, got HTTP {} with body {}",
            reply.status, reply.body
        );
    }

    let over = emulator
        .create_message_template(
            &knowledge_base_id,
            json!({
                "name": "one-too-many",
                "channelSubtype": "SMS",
                "content": content_for("SMS"),
            }),
        )
        .await;
    assert_eq!(
        over.status, 402,
        "case \"one past the ceiling\": expected HTTP 402, got HTTP {} with body {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_type, "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected error type ServiceQuotaExceededException, got \
         {:?}",
        over.error_type
    );
    assert!(
        over.string("/message")
            .contains(&MAX_MESSAGE_TEMPLATES_PER_KNOWLEDGE_BASE.to_string()),
        "case \"one past the ceiling\": the refusal must state the ceiling, got {:?}",
        over.string("/message")
    );
}

// ---------------------------------------------------------------------------------------
// `CreateMessageTemplateAttachment`: uploading one attachment file to a template's working
// copy.
//
// The rules under test, each asserted over a TABLE of inputs rather than the one recorded
// request:
//
// * an upload PERSISTS the attachment on the template the `$LATEST` qualifier names and
//   answers out of the stored record: a minted `attachmentId` of the modelled `Uuid` form,
//   the `name` and `contentDisposition` the request carried, an `uploadedTime`, and a
//   `url`/`urlExpiry` pair that addresses the file at the endpoint the caller reached — and
//   every later read of the template reports exactly that attachment, in upload order,
//   while the template's content checksum changes, since the model says attachments are
//   part of what it is calculated over;
// * that `url` is FOLLOWABLE: an unsigned `GET` answers the bytes the base64 body decoded
//   to, presented as the attachment the `contentDisposition` asked for, and the plane
//   serves nothing else;
// * an attachment `name` identifies one file on one working copy: a second upload claiming
//   it is `ConflictException` (HTTP 409) and leaves the first file untouched, while another
//   template may hold the same name;
// * a `body` that is no base64 encoding is `ValidationException` (HTTP 400) and persists
//   nothing;
// * a replayed `clientToken` answers with the attachment that token already uploaded,
//   while the same token carrying a different request is the modelled idempotency conflict;
// * a label that names no template of this knowledge base is `ResourceNotFoundException`
//   (HTTP 404), and one that carries a version qualifier is refused — the operation
//   documents the label as one that "cannot contain any qualifier";
// * one working copy holds at most the store's attachment ceiling, past which an upload is
//   the modelled `ServiceQuotaExceededException` (HTTP 402).

/// The only value the modelled `ContentDisposition` enumeration declares
/// (`smithy_query(query="shape",shape="ContentDisposition")`), written out here rather than
/// imported from the implementation.
const CONTENT_DISPOSITION: &str = "ATTACHMENT";

/// The published bound of `AttachmentFileName`
/// (`smithy_query(query="operation",operation="CreateMessageTemplateAttachment")`: `len
/// 1..256`, pattern `^[\p{L}\p{M}\p{N}_\s&@()+,;=\-]+\.[A-Za-z0-9]+$`).
const ATTACHMENT_NAME_MAX: usize = 256;

/// The sentence an upload whose `body` is not base64 is refused with, and the one an upload
/// past either attachment ceiling is refused with. Written out here rather than imported,
/// so the assertions pin what a client reads.
const MESSAGE_BODY_NOT_BASE64: &str = "The body must be encoded using base64 encoding.";
const MESSAGE_ATTACHMENT_QUOTA: &str =
    "This message template holds as many attachments as its store keeps. Delete an \
     attachment before uploading another.";


/// `bytes` as the base64 text the request's `body` carries, through the workspace's own
/// codec rather than a hand-written one.
fn base64_of(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

/// One instant a response reports, as the number of seconds since the epoch it names.
///
/// `MessageTemplateAttachment`'s `uploadedTime` and `urlExpiry` carry
/// `@timestampFormat("date-time")`, so what a response carries is an RFC 3339 STRING; this
/// reads that string back to an instant, which is what lets a row assert the INTERVAL
/// between two of them. `None` for an absent member and for a value that is no date-time
/// string at all — so a member reported in the protocol's default epoch-number form fails
/// the row rather than passing it.
fn instant(reply: &Reply, pointer: &str) -> Option<f64> {
    let text = reply.field(pointer)?.as_str()?.to_string();
    ::aws_smithy_types::DateTime::from_str(&text, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|parsed| parsed.as_secs_f64())
}

/// The attachment surface of the shared harness, and the fixtures these rows need.
impl Emulator {
    /// Upload one attachment to the template `message_template` names under
    /// `knowledge_base`.
    async fn create_attachment(
        &self,
        knowledge_base: &str,
        message_template: &str,
        body: Value,
    ) -> Reply {
        self.send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}/attachments",
                label(knowledge_base),
                label(message_template)
            ),
            Some(body),
        )
        .await
    }

    /// A message template of `knowledge_base` holding no attachment yet, answering with
    /// its `messageTemplateId`.
    async fn given_message_template(&self, knowledge_base: &str, name: &str) -> String {
        let created = self
            .create_message_template(
                knowledge_base,
                json!({
                    "name": name,
                    "channelSubtype": "EMAIL",
                    "content": content_for("EMAIL"),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the template these rows attach to must be created, got HTTP {} with \
             body {}",
            created.status, created.body
        );
        created.string("/messageTemplate/messageTemplateId")
    }
}

/// One attachment an upload carries. The rows differ along what the rule generalises over:
/// the file's bytes (text, binary, one byte, several kilobytes) and the form and length of
/// the name the pattern admits.
struct AttachmentCase {
    case: &'static str,
    name: String,
    file: Vec<u8>,
}

fn attachment_cases() -> Vec<AttachmentCase> {
    vec![
        AttachmentCase {
            case: "a text file",
            name: "notes.txt".to_string(),
            file: b"the attachment body, as a client uploaded it".to_vec(),
        },
        AttachmentCase {
            case: "a binary file whose bytes are no text",
            name: "logo.PNG".to_string(),
            file: vec![0x89, 0x50, 0x4e, 0x47, 0x00, 0x01, 0xfa, 0xff],
        },
        AttachmentCase {
            case: "a one-byte file, the shortest body the model admits",
            name: "x.y".to_string(),
            file: b"x".to_vec(),
        },
        AttachmentCase {
            // Every class of character the `AttachmentFileName` pattern admits beside the
            // letters: a mark, a number, `_`, whitespace and `&@()+,;=-`.
            case: "a name of every character class the pattern admits",
            name: "rapport final &@()+,;=-u\u{308}mlaut_2.pdf".to_string(),
            file: b"%PDF-1.7 and the rest".to_vec(),
        },
        AttachmentCase {
            case: "a name at the published maximum length",
            name: format!("{}.txt", "n".repeat(ATTACHMENT_NAME_MAX - 4)),
            file: b"at the name ceiling".to_vec(),
        },
        AttachmentCase {
            case: "a file of several kilobytes",
            name: "handbook.csv".to_string(),
            file: (0..4096u32).map(|index| (index % 251) as u8).collect(),
        },
    ]
}

/// The rule: an upload persists the attachment on the template's working copy and answers
/// out of the stored record, every later read reports it, and the `url` it reports can be
/// followed to download the file the base64 body decoded to.
#[tokio::test]
async fn an_upload_persists_the_attachment_and_its_url_serves_the_file() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("attachments").await;

    for case in attachment_cases() {
        let AttachmentCase { case, name, file } = case;
        let message_template_id = emulator
            .given_message_template(&knowledge_base_id, case)
            .await;
        let before = emulator
            .get_message_template(&knowledge_base_id, &message_template_id)
            .await;
        let checksum_before = before.string("/messageTemplate/messageTemplateContentSha256");
        assert!(
            before.field("/messageTemplate/attachments").is_none(),
            "case {case:?}: a template holds no attachment until one is uploaded, got {:?}",
            before.field("/messageTemplate/attachments")
        );

        let uploaded = emulator
            .create_attachment(
                &knowledge_base_id,
                &message_template_id,
                json!({
                    "name": name,
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": base64_of(&file),
                }),
            )
            .await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: expected HTTP 200 from the upload, got HTTP {} with body {}",
            uploaded.status, uploaded.body
        );
        let attachment_id = uploaded.string("/attachment/attachmentId");
        assert!(
            is_modelled_uuid(&attachment_id),
            "case {case:?}: expected a modelled Uuid attachmentId, got {attachment_id:?}"
        );
        assert_eq!(
            uploaded.string("/attachment/name"),
            name,
            "case {case:?}: the upload must report the name it carried, got {:?}",
            uploaded.string("/attachment/name")
        );
        assert_eq!(
            uploaded.string("/attachment/contentDisposition"),
            CONTENT_DISPOSITION,
            "case {case:?}: expected contentDisposition {CONTENT_DISPOSITION:?}, got {:?}",
            uploaded.string("/attachment/contentDisposition")
        );
        let url = uploaded.string("/attachment/url");
        super::content_upload::assert_signed_plane_url(
            case,
            &url,
            &format!(
                "/knowledgeBases/{knowledge_base_id}/messageTemplates/{message_template_id}\
                 /attachments/{attachment_id}/file"
            ),
            DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES * 60,
            REGION,
        );
        // Both instants are derived from the one instant the upload happened at, so the
        // url expires exactly the store's time to live after the file was uploaded.
        // Nothing here reads the test host's clock.
        let uploaded_time = instant(&uploaded, "/attachment/uploadedTime");
        let url_expiry = instant(&uploaded, "/attachment/urlExpiry");
        let (Some(uploaded_time), Some(url_expiry)) = (uploaded_time, url_expiry) else {
            panic!(
                "case {case:?}: the upload must report uploadedTime and urlExpiry, got {:?} and \
                 {:?}",
                uploaded.field("/attachment/uploadedTime"),
                uploaded.field("/attachment/urlExpiry")
            );
        };
        let time_to_live = (DEFAULT_PRESIGNED_URL_TIME_TO_LIVE_MINUTES * 60) as f64;
        // The signature the url carries is minted at a WHOLE second — SigV4 dates carry no
        // fraction — so the expiry is the time to live after the second the upload
        // happened in: at most the time to live after `uploadedTime`, and less than one
        // second short of it.
        let after_the_upload = url_expiry - uploaded_time;
        assert!(
            after_the_upload > time_to_live - 1.0 && after_the_upload <= time_to_live,
            "case {case:?}: the url must expire {time_to_live} seconds after the whole second \
             the upload at {uploaded_time} was signed in, got {url_expiry}"
        );

        // Every read of the template reports exactly that attachment — by bare id, by ARN
        // and under the `$LATEST` qualifier — and the checksum the model says is calculated
        // over the attachments has moved.
        let bare_arn = expected_message_template_arn(&knowledge_base_id, &message_template_id);
        let readers = [
            ("the bare id", message_template_id.clone()),
            ("the ARN", bare_arn.clone()),
            (
                "the $LATEST qualifier",
                format!("{message_template_id}:{QUALIFIER_LATEST}"),
            ),
        ];
        for (reader, template_label) in readers {
            let read = emulator
                .get_message_template(&knowledge_base_id, &template_label)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {reader}: expected HTTP 200, got HTTP {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.field("/messageTemplate/attachments").cloned(),
                uploaded
                    .field("/attachment")
                    .cloned()
                    .map(|attachment| Value::Array(vec![attachment])),
                "case {case:?} read by {reader}: the read must report the attachment the upload \
                 persisted, got {:?}",
                read.field("/messageTemplate/attachments")
            );
            let checksum_after = read.string("/messageTemplate/messageTemplateContentSha256");
            assert!(
                is_sha256_hex(&checksum_after),
                "case {case:?} read by {reader}: expected a SHA-256 checksum, got \
                 {checksum_after:?}"
            );
            assert_ne!(
                checksum_after, checksum_before,
                "case {case:?} read by {reader}: the checksum is calculated over the \
                 attachments, so uploading one must change it away from {checksum_before:?}"
            );
        }

        // The url is followable, unsigned, and answers the bytes the base64 body decoded
        // to, presented as an attachment with the file's own name.
        let followed = emulator.follow("GET", &url).await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: the attachment url must be followable, got HTTP {} with {:?}",
            followed.status,
            String::from_utf8_lossy(&followed.bytes)
        );
        assert_eq!(
            followed.bytes, file,
            "case {case:?}: the url must serve the file the body decoded to, got {} bytes",
            followed.bytes.len()
        );
        let disposition = followed
            .headers
            .get("content-disposition")
            .cloned()
            .unwrap_or_default();
        assert!(
            disposition.starts_with("attachment"),
            "case {case:?}: the file must be presented as the contentDisposition asked for, got \
             {disposition:?}"
        );
        if name.is_ascii() {
            assert!(
                disposition.contains(&format!("filename=\"{name}\"")),
                "case {case:?}: the presentation must name the file, got {disposition:?}"
            );
        }
        assert_eq!(
            followed.headers.get("content-length").cloned(),
            Some(file.len().to_string()),
            "case {case:?}: expected a content-length of {} bytes, got {:?}",
            file.len(),
            followed.headers.get("content-length")
        );

        // The plane reads the file and nothing else: another method against the same url is
        // refused, and the file is still there afterwards.
        let written = emulator.follow("PUT", &url).await;
        assert_eq!(
            written.status, 405,
            "case {case:?}: the attachment url is read by a GET alone, got HTTP {}",
            written.status
        );
        let again = emulator.follow("GET", &url).await;
        assert_eq!(
            again.bytes, file,
            "case {case:?}: the refused write must leave the file alone, got {} bytes",
            again.bytes.len()
        );
    }
}

/// The rule the operation documents: an attachment `name` identifies one file on one
/// working copy, so a second upload claiming it conflicts and leaves the first untouched,
/// while another template may hold the same name — and a template holds the attachments it
/// was given, in upload order.
#[tokio::test]
async fn an_attachment_name_identifies_one_file_on_the_working_copy() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("names").await;
    let first_template = emulator
        .given_message_template(&knowledge_base_id, "holder")
        .await;
    let other_template = emulator
        .given_message_template(&knowledge_base_id, "other-holder")
        .await;

    // Several names, uploaded in this order to ONE working copy.
    let names = ["first.txt", "second.csv", "third.pdf"];
    let mut ids = Vec::new();
    for name in names {
        let uploaded = emulator
            .create_attachment(
                &knowledge_base_id,
                &first_template,
                json!({
                    "name": name,
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": base64_of(format!("the file of {name}").as_bytes()),
                }),
            )
            .await;
        assert_eq!(
            uploaded.status, 200,
            "case {name:?}: expected HTTP 200, got HTTP {} with body {}",
            uploaded.status, uploaded.body
        );
        ids.push(uploaded.string("/attachment/attachmentId"));
    }

    for name in names {
        let again = emulator
            .create_attachment(
                &knowledge_base_id,
                &first_template,
                json!({
                    "name": name,
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": base64_of(b"a second file under a name in use"),
                }),
            )
            .await;
        assert_eq!(
            again.status, 409,
            "case {name:?}: a name this working copy holds must conflict, got HTTP {} with body \
             {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type, "ConflictException",
            "case {name:?}: expected error type ConflictException, got {:?}",
            again.error_type
        );
        assert_eq!(
            again.string("/message"),
            MESSAGE_NAME_IN_USE,
            "case {name:?}: expected the message {MESSAGE_NAME_IN_USE:?}, got {:?}",
            again.string("/message")
        );

        // Another template may hold the same name: the name identifies a file on ONE
        // working copy.
        let elsewhere = emulator
            .create_attachment(
                &knowledge_base_id,
                &other_template,
                json!({
                    "name": name,
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": base64_of(b"the same name, another template"),
                }),
            )
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: another template may hold the same attachment name, got HTTP {} with \
             body {}",
            elsewhere.status, elsewhere.body
        );
    }

    // The refused uploads left the working copy holding exactly the three files it was
    // given, in the order they were uploaded.
    let read = emulator
        .get_message_template(&knowledge_base_id, &first_template)
        .await;
    let reported: Vec<String> = read
        .field("/messageTemplate/attachments")
        .and_then(Value::as_array)
        .map(|attachments| {
            attachments
                .iter()
                .map(|attachment| {
                    attachment
                        .pointer("/attachmentId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, ids,
        "case \"three files in upload order\": expected the attachments {ids:?}, got {reported:?}"
    );
    let reported_names: Vec<String> = read
        .field("/messageTemplate/attachments")
        .and_then(Value::as_array)
        .map(|attachments| {
            attachments
                .iter()
                .map(|attachment| {
                    attachment
                        .pointer("/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported_names,
        names.map(str::to_string).to_vec(),
        "case \"three files in upload order\": expected the names {names:?}, got \
         {reported_names:?}"
    );
}

/// The rule the member documents: the `body` is base64, so a body that is no base64
/// encoding carries no file and is refused with the modelled `ValidationException`,
/// persisting nothing.
#[tokio::test]
async fn a_body_that_is_not_base64_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("bodies").await;
    let message_template_id = emulator
        .given_message_template(&knowledge_base_id, "bodies")
        .await;

    // Rows: characters outside the base64 alphabet, in each position a caller puts them.
    let bodies = [
        ("plain text that is no encoding", "not base64 at all!"),
        ("characters outside the alphabet", "%%%%"),
        ("an alphabet character replaced", "YW*jZA=="),
        ("padding in the middle", "YQ==YQ=="),
    ];
    for (case, body) in bodies {
        let refused = emulator
            .create_attachment(
                &knowledge_base_id,
                &message_template_id,
                json!({
                    "name": "refused.txt",
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": body,
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got HTTP {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected error type ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.string("/message"),
            MESSAGE_BODY_NOT_BASE64,
            "case {case:?}: expected the message {MESSAGE_BODY_NOT_BASE64:?}, got {:?}",
            refused.string("/message")
        );
    }

    let read = emulator
        .get_message_template(&knowledge_base_id, &message_template_id)
        .await;
    assert!(
        read.field("/messageTemplate/attachments").is_none(),
        "case \"every refused body\": a refused upload must persist nothing, got {:?}",
        read.field("/messageTemplate/attachments")
    );
}

/// The rule: a replayed `clientToken` answers with the attachment that token already
/// uploaded, while the same token carrying a different request is the modelled idempotency
/// conflict.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_attachment_it_uploaded() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("tokens").await;
    let message_template_id = emulator
        .given_message_template(&knowledge_base_id, "tokens")
        .await;
    let other_template = emulator
        .given_message_template(&knowledge_base_id, "tokens-elsewhere")
        .await;

    let token = "an-idempotency-token";
    let body = json!({
        "name": "receipt.pdf",
        "contentDisposition": CONTENT_DISPOSITION,
        "body": base64_of(b"the receipt"),
        "clientToken": token,
    });
    let first = emulator
        .create_attachment(&knowledge_base_id, &message_template_id, body.clone())
        .await;
    assert_eq!(
        first.status, 200,
        "case \"the first upload\": expected HTTP 200, got HTTP {} with body {}",
        first.status, first.body
    );

    let replayed = emulator
        .create_attachment(&knowledge_base_id, &message_template_id, body.clone())
        .await;
    assert_eq!(
        replayed.field("/attachment").cloned(),
        first.field("/attachment").cloned(),
        "case \"the identical request again\": a replay must answer with the attachment the \
         token uploaded, got {:?}",
        replayed.field("/attachment")
    );

    // Rows: the same token carrying a different request, one differing member each.
    let mut renamed = body.clone();
    renamed["name"] = json!("another.pdf");
    let mut refilled = body.clone();
    refilled["body"] = json!(base64_of(b"another file"));
    for (case, differing) in [("a different name", renamed), ("a different file", refilled)] {
        let conflict = emulator
            .create_attachment(&knowledge_base_id, &message_template_id, differing)
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {case:?}: expected HTTP 409, got HTTP {} with body {}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.string("/message"),
            MESSAGE_IDEMPOTENCY_CONFLICT,
            "case {case:?}: expected the message {MESSAGE_IDEMPOTENCY_CONFLICT:?}, got {:?}",
            conflict.string("/message")
        );
    }

    // The token is scoped to its own parent: the same token on another template uploads
    // its own attachment.
    let elsewhere = emulator
        .create_attachment(&knowledge_base_id, &other_template, body)
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"the same token on another template\": expected HTTP 200, got HTTP {} with body {}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        elsewhere.string("/attachment/attachmentId"),
        first.string("/attachment/attachmentId"),
        "case \"the same token on another template\": the two uploads must be two attachments"
    );

    // Exactly one attachment was stored on the first template, whatever the replays and
    // the refusals asked for.
    let read = emulator
        .get_message_template(&knowledge_base_id, &message_template_id)
        .await;
    assert_eq!(
        read.field("/messageTemplate/attachments")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case \"after the replays\": expected one stored attachment, got {:?}",
        read.field("/messageTemplate/attachments")
    );
}

/// The rule: an upload is addressed to a template of this knowledge base through a label
/// that carries no version qualifier — a label that names no such template is not found,
/// and one that carries a qualifier is refused.
#[tokio::test]
async fn an_upload_resolves_its_template_through_an_unqualified_label() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("labels").await;
    let other_base_id = emulator.create_knowledge_base_named("labels-elsewhere").await;
    let message_template_id = emulator
        .given_message_template(&knowledge_base_id, "labelled")
        .await;
    let elsewhere_id = emulator
        .given_message_template(&other_base_id, "held-elsewhere")
        .await;

    let body = json!({
        "name": "refused.txt",
        "contentDisposition": CONTENT_DISPOSITION,
        "body": base64_of(b"a file nobody stores"),
    });

    // Rows: every label form that names no template of THIS knowledge base.
    let missing = [
        ("an id no template holds", ABSENT_UUID.to_string()),
        (
            "an ARN of this account naming nothing",
            wisdom_arn(
                "aws",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &format!("message-template/{knowledge_base_id}/{ABSENT_UUID}"),
            ),
        ),
        (
            "an ARN of another account",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("message-template/{knowledge_base_id}/{message_template_id}"),
            ),
        ),
        (
            "another knowledge base's template",
            elsewhere_id.clone(),
        ),
    ];
    for (case, template_label) in missing {
        let refused = emulator
            .create_attachment(&knowledge_base_id, &template_label, body.clone())
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got HTTP {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ResourceNotFoundException",
            "case {case:?}: expected error type ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.string("/message"),
            MESSAGE_MESSAGE_TEMPLATE_NOT_FOUND,
            "case {case:?}: expected the message {MESSAGE_MESSAGE_TEMPLATE_NOT_FOUND:?}, got {:?}",
            refused.string("/message")
        );
    }

    // A knowledge base this account does not hold is reported as the missing resource.
    let no_parent = emulator
        .create_attachment(ABSENT_UUID, &message_template_id, body.clone())
        .await;
    assert_eq!(
        no_parent.status, 404,
        "case \"a knowledge base this account does not hold\": expected HTTP 404, got HTTP {} \
         with body {}",
        no_parent.status, no_parent.body
    );
    assert_eq!(
        no_parent.string("/message"),
        MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
        "case \"a knowledge base this account does not hold\": expected the message \
         {MESSAGE_KNOWLEDGE_BASE_NOT_FOUND:?}, got {:?}",
        no_parent.string("/message")
    );

    // Rows: every qualifier form the label may carry, each refused rather than read as
    // naming a version to upload to.
    for qualifier in [QUALIFIER_LATEST, QUALIFIER_ACTIVE, "1"] {
        let refused = emulator
            .create_attachment(
                &knowledge_base_id,
                &format!("{message_template_id}:{qualifier}"),
                body.clone(),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {qualifier:?}: expected HTTP 400, got HTTP {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {qualifier:?}: expected error type ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.string("/message"),
            MESSAGE_QUALIFIER_NOT_ACCEPTED,
            "case {qualifier:?}: expected the message {MESSAGE_QUALIFIER_NOT_ACCEPTED:?}, got \
             {:?}",
            refused.string("/message")
        );
    }

    // None of the refusals stored anything.
    let read = emulator
        .get_message_template(&knowledge_base_id, &message_template_id)
        .await;
    assert!(
        read.field("/messageTemplate/attachments").is_none(),
        "case \"every refused label\": a refused upload must persist nothing, got {:?}",
        read.field("/messageTemplate/attachments")
    );
}

/// The rule: a file is stored up to the store's per-file ceiling, and a body past it is
/// refused with the modelled `ValidationException` stating the limit — the bound is tested
/// at the ceiling and one byte past it.
#[tokio::test]
async fn a_file_at_the_per_file_ceiling_is_stored_and_one_past_it_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("file-ceiling").await;
    let message_template_id = emulator
        .given_message_template(&knowledge_base_id, "file-ceiling")
        .await;

    let at_ceiling: Vec<u8> = (0..MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES)
        .map(|index| (index % 256) as u8)
        .collect();
    let accepted = emulator
        .create_attachment(
            &knowledge_base_id,
            &message_template_id,
            json!({
                "name": "at-the-ceiling.bin",
                "contentDisposition": CONTENT_DISPOSITION,
                "body": base64_of(&at_ceiling),
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a file at the ceiling\": expected HTTP 200, got HTTP {} with body {}",
        accepted.status, accepted.body
    );
    let followed = emulator
        .follow("GET", &accepted.string("/attachment/url"))
        .await;
    assert_eq!(
        followed.bytes.len(),
        at_ceiling.len(),
        "case \"a file at the ceiling\": the whole file must be served, got {} bytes",
        followed.bytes.len()
    );

    let past_ceiling: Vec<u8> = (0..=MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES)
        .map(|index| (index % 256) as u8)
        .collect();
    let refused = emulator
        .create_attachment(
            &knowledge_base_id,
            &message_template_id,
            json!({
                "name": "one-byte-too-far.bin",
                "contentDisposition": CONTENT_DISPOSITION,
                "body": base64_of(&past_ceiling),
            }),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"one byte past the ceiling\": expected HTTP 400, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ValidationException",
        "case \"one byte past the ceiling\": expected error type ValidationException, got {:?}",
        refused.error_type
    );
    assert!(
        refused
            .string("/message")
            .contains(&MAX_MESSAGE_TEMPLATE_ATTACHMENT_BYTES.to_string()),
        "case \"one byte past the ceiling\": the refusal must state the limit, got {:?}",
        refused.string("/message")
    );

    // The refusal left the stored file alone.
    let read = emulator
        .get_message_template(&knowledge_base_id, &message_template_id)
        .await;
    assert_eq!(
        read.field("/messageTemplate/attachments")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case \"one byte past the ceiling\": expected the one stored attachment, got {:?}",
        read.field("/messageTemplate/attachments")
            .and_then(Value::as_array)
            .map(Vec::len)
    );
}

/// The rule: one working copy holds at most the store's attachment ceiling, past which an
/// upload is the modelled `ServiceQuotaExceededException` (HTTP 402) and nothing is stored.
#[tokio::test]
async fn an_upload_past_the_attachment_ceiling_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator.create_knowledge_base_named("ceiling").await;
    let message_template_id = emulator
        .given_message_template(&knowledge_base_id, "ceiling")
        .await;

    for index in 0..MAX_ATTACHMENTS_PER_MESSAGE_TEMPLATE {
        let uploaded = emulator
            .create_attachment(
                &knowledge_base_id,
                &message_template_id,
                json!({
                    "name": format!("bulk-{index}.txt"),
                    "contentDisposition": CONTENT_DISPOSITION,
                    "body": base64_of(format!("the file of {index}").as_bytes()),
                }),
            )
            .await;
        assert_eq!(
            uploaded.status, 200,
            "case \"attachment {index}\": expected HTTP 200 up to the ceiling, got HTTP {} with \
             body {}",
            uploaded.status, uploaded.body
        );
    }

    let over = emulator
        .create_attachment(
            &knowledge_base_id,
            &message_template_id,
            json!({
                "name": "one-too-many.txt",
                "contentDisposition": CONTENT_DISPOSITION,
                "body": base64_of(b"one file too many"),
            }),
        )
        .await;
    assert_eq!(
        over.status, 402,
        "case \"one past the ceiling\": expected HTTP 402, got HTTP {} with body {}",
        over.status, over.body
    );
    assert_eq!(
        over.error_type, "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected error type ServiceQuotaExceededException, got \
         {:?}",
        over.error_type
    );
    assert_eq!(
        over.string("/message"),
        MESSAGE_ATTACHMENT_QUOTA,
        "case \"one past the ceiling\": expected the message {MESSAGE_ATTACHMENT_QUOTA:?}, got \
         {:?}",
        over.string("/message")
    );

    let read = emulator
        .get_message_template(&knowledge_base_id, &message_template_id)
        .await;
    assert_eq!(
        read.field("/messageTemplate/attachments")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(MAX_ATTACHMENTS_PER_MESSAGE_TEMPLATE),
        "case \"one past the ceiling\": the working copy must hold exactly the ceiling, got {:?}",
        read.field("/messageTemplate/attachments")
            .and_then(Value::as_array)
            .map(Vec::len)
    );
}
