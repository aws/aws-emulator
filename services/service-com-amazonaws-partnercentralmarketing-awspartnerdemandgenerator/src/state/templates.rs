// SPDX-License-Identifier: CC0-1.0

//! The campaign-template catalog this emulator serves, and the search over it.
//!
//! A campaign template is "a platform-managed campaign template — an AWS-authored kit
//! of asset blueprints a partner creates a campaign from": there is no create, update
//! or delete operation for one, and every partner reads the same catalog. So the
//! catalog is a CONSTANT here rather than a seed written into the engine's store — a
//! read of it writes nothing, and two emulators, or two callers of one emulator, cannot
//! see different catalogs.
//!
//! No member of a template carries a URL. A thumbnail, a layout preview and a reference
//! material are each a file AWS serves from its own CDN; this emulator serves no bytes
//! for them, so it announces no address a client would fetch and find nothing at.

use std::time::SystemTime;

use service_common::document_text_search::{query_terms, SearchableText};

use super::*;
use crate::ops::types::{
    CampaignAssetTemplate, CampaignAssetTemplateField, CampaignTemplate, CampaignTemplateSummary,
    FieldType, ReferenceMaterial, RichTextFieldType, TextFieldType,
};

/// How many characters the LAST word of a `SearchText` needs before it matches as a
/// prefix, from `SearchCampaignTemplatesRequest$SearchText` ("the last word matches as
/// a prefix when it has 3 or more characters, so `nov` finds Amazon Nova").
const PREFIX_MATCH_MIN_CHARS: usize = 3;

/// One template of the catalog, exactly as the catalog holds it.
///
/// Borrowed rather than owned: the catalog is a constant, so a read projects it onto the
/// wire shapes without copying it into the store first.
#[derive(Debug, Clone, Copy)]
pub struct CampaignTemplateRecord {
    /// The `CampaignTemplateId` (`tmpl-` + 13 characters of `[0-9a-z]`).
    pub id: &'static str,
    pub name: &'static str,
    /// `Industry`, one of [`crate::ops::types::INDUSTRY_VALUES`]; absent when the
    /// template names none.
    pub industry: Option<&'static str>,
    pub use_case: Option<&'static str>,
    pub description: &'static str,
    /// `LanguageCode`, one of [`crate::ops::types::ASSET_LANGUAGE_VALUES`] — the
    /// language a campaign created from this template takes.
    pub language_code: &'static str,
    /// `Collection`, one of
    /// [`crate::ops::types::CAMPAIGN_TEMPLATE_COLLECTION_VALUES`].
    pub collection: &'static str,
    pub aws_services: &'static [&'static str],
    /// The customer segments the template is written for. Empty when it names none, in
    /// which case it "matches every `CustomerSegments` search filter".
    pub customer_segments: &'static [&'static str],
    /// The template's marketing themes, with the same rule for an empty list.
    pub themes: &'static [&'static str],
    pub setup_duration: &'static str,
    /// `EffectiveDateBegin` as an RFC 3339 instant: the instant the template becomes
    /// available to partners. Until then "the template does not exist to a partner".
    pub effective_date_begin: &'static str,
    pub created_at: &'static str,
    pub updated_at: &'static str,
    pub asset_templates: &'static [AssetTemplateSeed],
    /// The reference materials' display names. Their `Url`s are withheld for the
    /// reason this module's header gives.
    pub reference_materials: &'static [&'static str],
}

/// One asset blueprint of a template: the id is template-local, so the same blueprint
/// appears in several templates under the same id.
#[derive(Debug, Clone, Copy)]
pub struct AssetTemplateSeed {
    pub id: &'static str,
    /// `CampaignAssetTemplateType`, one of
    /// [`crate::ops::types::CAMPAIGN_ASSET_TEMPLATE_TYPE_VALUES`].
    pub asset_type: &'static str,
    pub label: &'static str,
    pub display_order: i32,
    pub fields: &'static [FieldSeed],
}

/// One editable slot of an asset blueprint.
#[derive(Debug, Clone, Copy)]
pub struct FieldSeed {
    pub field_id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// `TemplateFieldSection`, one of
    /// [`crate::ops::types::TEMPLATE_FIELD_SECTION_VALUES`].
    pub section: &'static str,
    pub required: bool,
    /// Whether the slot carries plain text or rich text, and how much of it.
    pub rich: bool,
    pub max_length: i32,
    /// The `CampaignSetting` this slot shows, when it shows one — which is what makes
    /// an unset campaign setting an activation issue naming the assets made from this
    /// blueprint.
    pub campaign_setting: Option<&'static str>,
}

/// The editable slots of an email asset: its own copy, plus the two campaign settings
/// every email of a campaign displays.
const EMAIL_FIELDS: &[FieldSeed] = &[
    FieldSeed {
        field_id: "Subject",
        label: "Subject line",
        description: "The subject line, naming the outcome the reader gets.",
        section: "HEADER",
        required: true,
        rich: false,
        max_length: 150,
        campaign_setting: None,
    },
    FieldSeed {
        field_id: "BodyCopy",
        label: "Body copy",
        description: "The body of the email, in the template's language.",
        section: "BODY",
        required: true,
        rich: true,
        max_length: 4000,
        campaign_setting: None,
    },
    FieldSeed {
        field_id: "SenderAddress",
        label: "Sender address",
        description: "The address this email is sent from; set once on the campaign.",
        section: "HEADER",
        required: true,
        rich: false,
        max_length: 254,
        campaign_setting: Some("SENDER_EMAIL"),
    },
    FieldSeed {
        field_id: "FooterAddress",
        label: "Postal address",
        description: "The partner's postal address, shown in the footer; set once on the campaign.",
        section: "FOOTER",
        required: true,
        rich: false,
        max_length: 200,
        campaign_setting: Some("PARTNER_ADDRESS"),
    },
];

/// The editable slots of a landing page.
const LANDING_PAGE_FIELDS: &[FieldSeed] = &[
    FieldSeed {
        field_id: "Headline",
        label: "Headline",
        description: "The headline the page opens with.",
        section: "HEADER",
        required: true,
        rich: false,
        max_length: 120,
        campaign_setting: None,
    },
    FieldSeed {
        field_id: "Overview",
        label: "Overview",
        description: "What the reader gets from the offer, in the template's language.",
        section: "BODY",
        required: true,
        rich: true,
        max_length: 3000,
        campaign_setting: None,
    },
    FieldSeed {
        field_id: "PrivacyPolicy",
        label: "Privacy policy link",
        description: "The partner's privacy policy, linked in the footer; set once on the campaign.",
        section: "FOOTER",
        required: true,
        rich: false,
        max_length: 2048,
        campaign_setting: Some("PRIVACY_POLICY_URL"),
    },
];

/// The editable slots of a social post.
const SOCIAL_POST_FIELDS: &[FieldSeed] = &[FieldSeed {
    field_id: "PostText",
    label: "Post text",
    description: "The text of the post.",
    section: "BODY",
    required: true,
    rich: false,
    max_length: 280,
    campaign_setting: None,
}];

/// The asset blueprints an outbound-email template carries.
const EMAIL_AND_PAGE: &[AssetTemplateSeed] = &[
    AssetTemplateSeed {
        id: "announcement-email",
        asset_type: "EMAIL",
        label: "Announcement email",
        display_order: 1,
        fields: EMAIL_FIELDS,
    },
    AssetTemplateSeed {
        id: "offer-landing-page",
        asset_type: "LANDING_PAGE",
        label: "Offer landing page",
        display_order: 2,
        fields: LANDING_PAGE_FIELDS,
    },
];

/// The asset blueprints a social-first template carries.
const SOCIAL_AND_PAGE: &[AssetTemplateSeed] = &[
    AssetTemplateSeed {
        id: "awareness-social-post",
        asset_type: "SOCIAL_POST",
        label: "Awareness social post",
        display_order: 1,
        fields: SOCIAL_POST_FIELDS,
    },
    AssetTemplateSeed {
        id: "offer-landing-page",
        asset_type: "LANDING_PAGE",
        label: "Offer landing page",
        display_order: 2,
        fields: LANDING_PAGE_FIELDS,
    },
];

/// The instant the catalog's UPCOMING template becomes available to partners.
///
/// Deliberately far ahead of any run of this emulator: it is the one template the
/// catalog carries whose `EffectiveDateBegin` has not been reached, so the documented
/// rule — a search omits it and a read of it answers `ResourceNotFoundException` — is
/// exercised by the catalog itself rather than only by a comment.
const UPCOMING_EFFECTIVE_DATE: &str = "2200-01-01T00:00:00Z";

/// The campaign templates this emulator serves, in the catalog's own browsing order.
///
/// One row per combination a search filter distinguishes — the two collections, several
/// industries, three languages, templates with and without customer segments and
/// themes — so every facet has something to match and something to exclude.
const CATALOG: &[CampaignTemplateRecord] = &[
    CampaignTemplateRecord {
        id: "tmpl-migration0001",
        name: "Accelerate Windows migration",
        industry: Some("MANUFACTURING"),
        use_case: Some("Migration"),
        description: "Reach customers still running self-managed Windows workloads and offer a \
                      migration assessment that reviews their security posture.",
        language_code: "en",
        collection: "GENERAL",
        aws_services: &["Amazon EC2", "AWS Migration Hub"],
        customer_segments: &["ENTERPRISE"],
        themes: &["MIGRATION_MODERNIZATION"],
        setup_duration: "PT2H",
        effective_date_begin: "2026-01-05T00:00:00Z",
        created_at: "2025-11-18T09:30:00Z",
        updated_at: "2026-01-05T00:00:00Z",
        asset_templates: EMAIL_AND_PAGE,
        reference_materials: &["Migration readiness checklist", "Customer-ready slide deck"],
    },
    CampaignTemplateRecord {
        id: "tmpl-security00001",
        name: "Security posture review",
        industry: Some("FINANCIAL_SERVICES"),
        use_case: Some("Security assessment"),
        description: "Offer a posture review to customers tightening their cloud controls.",
        language_code: "en",
        collection: "GENERAL",
        aws_services: &["AWS Security Hub", "Amazon GuardDuty"],
        customer_segments: &["ENTERPRISE", "PUBLIC_SECTOR"],
        themes: &["SECURITY"],
        setup_duration: "PT90M",
        effective_date_begin: "2026-02-02T00:00:00Z",
        created_at: "2025-12-01T11:00:00Z",
        updated_at: "2026-02-02T00:00:00Z",
        asset_templates: EMAIL_AND_PAGE,
        reference_materials: &["Posture review workbook"],
    },
    CampaignTemplateRecord {
        id: "tmpl-genai00000001",
        name: "Build with Amazon Nova",
        industry: None,
        use_case: Some("Generative AI"),
        description: "Introduce customers to generative AI applications built on Amazon Nova \
                      models.",
        language_code: "en",
        collection: "BUILD_WITH_AI",
        aws_services: &["Amazon Bedrock"],
        // Names no customer segment and no theme, so it matches every value of either
        // filter.
        customer_segments: &[],
        themes: &[],
        setup_duration: "PT45M",
        effective_date_begin: "2026-03-09T00:00:00Z",
        created_at: "2026-01-20T08:15:00Z",
        updated_at: "2026-03-09T00:00:00Z",
        asset_templates: SOCIAL_AND_PAGE,
        reference_materials: &["Model selection guide"],
    },
    CampaignTemplateRecord {
        id: "tmpl-dataanalytics",
        name: "Modernise your data platform",
        industry: Some("RETAIL"),
        use_case: Some("Data analytics"),
        description: "Move retail reporting off nightly batch jobs and onto a managed lakehouse.",
        language_code: "es",
        collection: "GENERAL",
        aws_services: &["Amazon Redshift", "AWS Glue"],
        customer_segments: &["SMB"],
        themes: &["DATA_ANALYTICS"],
        setup_duration: "PT3H",
        effective_date_begin: "2026-02-16T00:00:00Z",
        created_at: "2025-10-07T14:45:00Z",
        updated_at: "2026-02-16T00:00:00Z",
        asset_templates: EMAIL_AND_PAGE,
        reference_materials: &["Lakehouse reference architecture"],
    },
    CampaignTemplateRecord {
        id: "tmpl-marketplace01",
        name: "Launch your AWS Marketplace listing",
        industry: Some("OTHER"),
        use_case: Some("Marketplace listing"),
        description: "Announce a new AWS Marketplace listing to the customers already buying \
                      through it.",
        language_code: "ja",
        collection: "GENERAL",
        aws_services: &["AWS Marketplace"],
        customer_segments: &["ISV"],
        themes: &["AWS_MARKETPLACE"],
        setup_duration: "PT1H",
        effective_date_begin: "2026-01-19T00:00:00Z",
        created_at: "2025-09-30T10:00:00Z",
        updated_at: "2026-01-19T00:00:00Z",
        asset_templates: SOCIAL_AND_PAGE,
        reference_materials: &["Listing launch checklist"],
    },
    CampaignTemplateRecord {
        id: "tmpl-trainingcert1",
        name: "Certify your practice",
        industry: Some("EDUCATION"),
        use_case: Some("Training and certification"),
        description: "Fill a training cohort by offering certification paths to customer teams.",
        language_code: "en",
        collection: "GENERAL",
        aws_services: &["AWS Training and Certification"],
        customer_segments: &["ENTERPRISE", "SMB"],
        themes: &["AWS_TRAINING_CERTIFICATION"],
        setup_duration: "PT30M",
        effective_date_begin: "2026-01-12T00:00:00Z",
        created_at: "2025-11-02T16:20:00Z",
        updated_at: "2026-01-12T00:00:00Z",
        asset_templates: EMAIL_AND_PAGE,
        reference_materials: &["Cohort invitation copy"],
    },
    CampaignTemplateRecord {
        id: "tmpl-upcoming00001",
        name: "Agentic automation launch",
        industry: Some("TELECOMMUNICATIONS"),
        use_case: Some("Generative AI"),
        description: "The next Build with AI kit, published when its effective date is reached.",
        language_code: "en",
        collection: "BUILD_WITH_AI",
        aws_services: &["Amazon Bedrock"],
        customer_segments: &["ENTERPRISE"],
        themes: &["AI"],
        setup_duration: "PT1H",
        effective_date_begin: UPCOMING_EFFECTIVE_DATE,
        created_at: "2026-04-01T09:00:00Z",
        updated_at: "2026-04-01T09:00:00Z",
        asset_templates: SOCIAL_AND_PAGE,
        reference_materials: &[],
    },
];

/// The whole catalog, in its own browsing order — including any template whose
/// `EffectiveDateBegin` has not been reached, which [`effective_templates`] filters out.
pub fn catalog() -> &'static [CampaignTemplateRecord] {
    CATALOG
}

/// Whether `record` is available to a partner at `now`: its `EffectiveDateBegin` has
/// been reached.
///
/// A template whose effective instant cannot be read as one is treated as available,
/// which is the same answer the model's own optional member gives for a template that
/// declares no publication instant.
pub fn is_effective(record: &CampaignTemplateRecord, now: SystemTime) -> bool {
    let Some(effective) = parse_instant(record.effective_date_begin) else {
        return true;
    };
    let now = timestamp(now);
    (effective.secs(), effective.subsec_nanos()) <= (now.secs(), now.subsec_nanos())
}

/// The templates a partner can see at `now`, in the catalog's own order.
pub fn effective_templates(now: SystemTime) -> Vec<&'static CampaignTemplateRecord> {
    CATALOG
        .iter()
        .filter(|record| is_effective(record, now))
        .collect()
}

/// The template `identifier` names, or `None` when the catalog holds none a partner can
/// see at `now` — an id no template carries, and one whose `EffectiveDateBegin` is in
/// the future, which "does not exist to a partner" until then.
pub fn effective_template(
    identifier: &str,
    now: SystemTime,
) -> Option<&'static CampaignTemplateRecord> {
    CATALOG
        .iter()
        .find(|record| record.id == identifier && is_effective(record, now))
}

/// Whether `record` passes every facet filter the search named.
///
/// "Values within this member are OR-ed; distinct facet members are AND-ed", with the
/// two facets whose own documentation says an unlabelled template matches every value
/// (`CustomerSegments`, `Themes`) treated that way and the rest as an exact,
/// case-sensitive match.
pub fn passes_facets(
    record: &CampaignTemplateRecord,
    request: &crate::ops::types::SearchCampaignTemplatesInput,
) -> bool {
    names_one_of(request.industry.as_ref(), record.industry.into_iter())
        && names_one_of(request.use_case.as_ref(), record.use_case.into_iter())
        && names_one_of(
            request.language.as_ref(),
            std::iter::once(record.language_code),
        )
        && names_one_of(
            request.aws_services.as_ref(),
            record.aws_services.iter().copied(),
        )
        && names_one_of(
            request.collection.as_ref(),
            std::iter::once(record.collection),
        )
        && unlabelled_or_names_one_of(request.customer_segments.as_ref(), record.customer_segments)
        && unlabelled_or_names_one_of(request.themes.as_ref(), record.themes)
}

/// Whether the template's values for one facet include a value the filter named, or the
/// filter was not given at all.
fn names_one_of<'value>(
    filter: Option<&Vec<String>>,
    mut values: impl Iterator<Item = &'value str>,
) -> bool {
    match filter {
        None => true,
        Some(wanted) => values.any(|value| wanted.iter().any(|want| want == value)),
    }
}

/// The same, for a facet whose documentation says a template naming NO value of it
/// matches every value of the filter.
fn unlabelled_or_names_one_of(filter: Option<&Vec<String>>, values: &[&str]) -> bool {
    values.is_empty() || names_one_of(filter, values.iter().copied())
}

/// How well a template answers a free-text query, highest first.
///
/// The ranking `SearchCampaignTemplatesRequest$SearchText` documents: "A template whose
/// name holds the query as a phrase ranks first, then whole-word matches, then
/// prefix-only ones."
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SearchRank {
    /// The template's name holds the query's words as a phrase.
    phrase_in_name: bool,
    /// How many of the query's words the template carries as whole words.
    whole_words: usize,
    /// How many it carries only as the prefix of a word.
    prefixes: usize,
}

/// The rank `record` answers a query of `terms` with, or `None` when it carries none of
/// them.
///
/// The query is matched against "the template's name and description", word by word:
/// "any word may match, and the last word matches as a prefix when it has 3 or more
/// characters". The words of both sides are the ONE tokenisation this workspace reads a
/// query with, so the caller's punctuation and letter case need not be reproduced.
pub fn search_rank(record: &CampaignTemplateRecord, terms: &[String]) -> Option<SearchRank> {
    if terms.is_empty() {
        return None;
    }
    let mut words = query_terms(record.name);
    words.extend(query_terms(record.description));
    let last = terms.len() - 1;
    let mut whole_words = 0usize;
    let mut prefixes = 0usize;
    for (position, term) in terms.iter().enumerate() {
        if words.iter().any(|word| word == term) {
            whole_words += 1;
        } else if position == last
            && term.chars().count() >= PREFIX_MATCH_MIN_CHARS
            && words.iter().any(|word| word.starts_with(term.as_str()))
        {
            prefixes += 1;
        }
    }
    if whole_words == 0 && prefixes == 0 {
        return None;
    }
    Some(SearchRank {
        // The query's words in order, which is the phrase the caller asked for however
        // they spaced and punctuated it.
        phrase_in_name: SearchableText::new(record.name).carries(&terms.join(" ")),
        whole_words,
        prefixes,
    })
}

/// The distinct words a `SearchText` names, folded for matching.
pub fn search_terms(search_text: &str) -> Vec<String> {
    query_terms(search_text)
}

/// The stored template as the wire `CampaignTemplate` shape, member for member.
pub fn template_shape(record: &CampaignTemplateRecord) -> CampaignTemplate {
    CampaignTemplate {
        id: record.id.to_string(),
        name: record.name.to_string(),
        industry: record.industry.map(str::to_string),
        use_case: record.use_case.map(str::to_string),
        description: Some(record.description.to_string()),
        language_code: Some(record.language_code.to_string()),
        collection: record.collection.to_string(),
        aws_services: list_of(record.aws_services),
        customer_segments: list_of(record.customer_segments),
        themes: list_of(record.themes),
        setup_duration: Some(record.setup_duration.to_string()),
        // Withheld: this emulator serves no file for a template, so it announces no
        // address for one.
        thumbnail_url: None,
        effective_date_begin: parse_instant(record.effective_date_begin),
        asset_templates: Some(
            record
                .asset_templates
                .iter()
                .map(asset_template_shape)
                .collect(),
        ),
        reference_materials: Some(
            record
                .reference_materials
                .iter()
                .map(|name| ReferenceMaterial {
                    name: (*name).to_string(),
                    url: None,
                })
                .collect(),
        ),
        created_at: parse_instant(record.created_at),
        updated_at: parse_instant(record.updated_at),
    }
}

/// The stored template as the wire `CampaignTemplateSummary` shape — the subset a search
/// reports.
pub fn template_summary_shape(record: &CampaignTemplateRecord) -> CampaignTemplateSummary {
    CampaignTemplateSummary {
        id: record.id.to_string(),
        name: record.name.to_string(),
        industry: record.industry.map(str::to_string),
        use_case: record.use_case.map(str::to_string),
        description: Some(record.description.to_string()),
        language_code: Some(record.language_code.to_string()),
        collection: record.collection.to_string(),
        aws_services: list_of(record.aws_services),
        customer_segments: list_of(record.customer_segments),
        themes: list_of(record.themes),
        thumbnail_url: None,
    }
}

/// One asset blueprint as the wire `CampaignAssetTemplate` shape.
fn asset_template_shape(seed: &AssetTemplateSeed) -> CampaignAssetTemplate {
    CampaignAssetTemplate {
        id: seed.id.to_string(),
        r#type: seed.asset_type.to_string(),
        label: seed.label.to_string(),
        display_order: Some(seed.display_order),
        fields: Some(seed.fields.iter().map(field_shape).collect()),
        // Withheld for the reason `thumbnail_url` is: a preview is a file, and this
        // emulator serves none.
        layout_previews: None,
        prerequisite_asset_template_ids: None,
        social_platform: None,
    }
}

/// One editable slot as the wire `CampaignAssetTemplateField` shape.
fn field_shape(seed: &FieldSeed) -> CampaignAssetTemplateField {
    CampaignAssetTemplateField {
        field_id: seed.field_id.to_string(),
        label: Some(seed.label.to_string()),
        description: Some(seed.description.to_string()),
        section: Some(seed.section.to_string()),
        page: None,
        required: Some(seed.required),
        r#type: if seed.rich {
            FieldType::RichText(RichTextFieldType {
                max_length: Some(seed.max_length),
                allowed_tags: vec!["p".to_string(), "br".to_string(), "strong".to_string()],
                placeholder: None,
            })
        } else {
            FieldType::Text(TextFieldType {
                min_length: Some(1),
                max_length: Some(seed.max_length),
                pattern: None,
                placeholder: None,
            })
        },
        campaign_setting: seed.campaign_setting.map(str::to_string),
        partner_profile_field: None,
    }
}

/// A list member of a template, absent rather than empty when the template names
/// nothing for it: "Absent when the template names none".
fn list_of(values: &[&str]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.iter().map(|value| (*value).to_string()).collect())
}
