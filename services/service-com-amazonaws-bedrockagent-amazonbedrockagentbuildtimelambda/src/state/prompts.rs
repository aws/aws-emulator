// SPDX-License-Identifier: CC0-1.0

//! The prompt slice of this service's state: the record `CreatePrompt` stores and
//! `GetPrompt` reports, the single projections off that record, and the one place a
//! `PromptIdentifier` is resolved.
//!
//! A prompt's DRAFT version IS the stored record: `CreatePrompt$version` is documented
//! "When you create a prompt, the version created is the DRAFT version", and the
//! working draft is what every later change to the prompt moves — so the record carries
//! the prompt's own `version` member and nothing copies the draft beside it. A NUMBERED
//! version is a snapshot cut from that draft, and the store holds one only once
//! something has cut it, so the versions a prompt has are the DRAFT it is plus whatever
//! was cut from it.
//!
//! The tags a prompt holds live on this record too, so the tagging surface
//! (`tagged_resources` beside this module) reports the tags `CreatePrompt` stored rather
//! than a second copy of them.

use super::*;

use crate::ops::types::{
    CreatePromptOutput, CreatePromptVersionOutput, GetPromptOutput, PromptVariant,
    UpdatePromptOutput,
};

/// The most prompts this store holds; past it `CreatePrompt` answers the modelled
/// `ServiceQuotaExceededException` instead of growing.
///
/// Amazon Bedrock publishes an adjustable quota on prompts per account per Region, and
/// this emulator models neither Service Quotas nor an account's adjustments — so this is
/// the emulator's OWN bound on the store, set far above any workflow an emulator serves,
/// and it is what stops the prompt state growing without end.
pub const PROMPT_QUOTA: usize = 1_000;

/// One stored prompt: the single source of truth for every operation that reports a
/// prompt's working draft.
#[derive(Debug, Clone)]
pub struct PromptRecord {
    /// The prompt's own identifier, as the modelled `PromptId` (`^[0-9a-zA-Z]{10}$`)
    /// carries it.
    pub prompt_id: String,
    /// The ARN of the prompt, as the modelled `PromptArn` spells it
    /// (`…:prompt/<promptId>`); a numbered version qualifies it with its number.
    pub arn: String,
    pub name: String,
    /// The version this record IS: the prompt's working draft.
    pub version: String,
    pub description: Option<String>,
    pub customer_encryption_key_arn: Option<String>,
    pub default_variant: Option<String>,
    /// The variants the prompt carries, exactly as the request that wrote them left
    /// them — an ABSENT list and an empty one are different values of the member, and
    /// the modelled `PromptVariantList` is `length ..1`, so there is at most one.
    pub variants: Option<Vec<PromptVariant>>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The highest number any version cut from this prompt has ever carried: a
    /// high-water mark, so the next cut takes the next number even if the version that
    /// held the last one has since been deleted. Zero until something cuts one.
    pub versions_cut: u32,
}

impl PromptRecord {
    /// This record as the `CreatePrompt` response: every member comes from the record
    /// just stored, never from the request being served.
    pub fn to_create_output(&self) -> CreatePromptOutput {
        CreatePromptOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            default_variant: self.default_variant.clone(),
            variants: self.variants.clone(),
            id: self.prompt_id.clone(),
            arn: self.arn.clone(),
            version: self.version.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// This record as the `UpdatePrompt` response: every member comes from the record as
    /// the update left it, never from the request being served. The version reported is
    /// the record's own, which is the working draft — `UpdatePrompt$version` is documented
    /// "When you update a prompt, the version updated is the DRAFT version" — and the ARN
    /// is the prompt's own, unqualified, because the draft is what moved.
    pub fn to_update_output(&self) -> UpdatePromptOutput {
        UpdatePromptOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            default_variant: self.default_variant.clone(),
            variants: self.variants.clone(),
            id: self.prompt_id.clone(),
            arn: self.arn.clone(),
            version: self.version.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// This record as the `GetPrompt` response. `with_variants` is false for a
    /// metadata-only read, which drops the prompt's variants and nothing else.
    pub fn to_get_output(&self, with_variants: bool) -> GetPromptOutput {
        GetPromptOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            default_variant: self.default_variant.clone(),
            variants: with_variants.then(|| self.variants.clone()).flatten(),
            id: self.prompt_id.clone(),
            arn: self.arn.clone(),
            version: self.version.clone(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }
}

/// The most numbered versions this store holds for one prompt; past it
/// `CreatePromptVersion` answers the modelled `ServiceQuotaExceededException` instead of
/// growing.
///
/// Amazon Bedrock publishes an adjustable quota on versions per prompt, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the emulator's
/// OWN bound on the store, set far above any workflow an emulator serves, and it is what
/// stops one prompt's version state growing without end.
pub const PROMPT_VERSION_QUOTA: usize = 1_000;

/// One stored numbered version of a prompt: the single source of truth for every
/// operation that reports a version cut from the prompt's working draft.
///
/// A numbered version is a STATIC SNAPSHOT of that draft at the instant
/// `CreatePromptVersion` cut it (the operation is documented "Creates a static snapshot
/// of your prompt that can be deployed to production"), so the members that make up the
/// snapshot are stored here rather than read back off the prompt: a later change to the
/// draft moves the draft alone.
///
/// The version number is held as the integer it counts rather than as the wire string, so
/// one prompt's versions order by the sequence they were cut in
/// (`CreatePromptVersion$version` is documented "Versions are numbered incrementally,
/// starting from 1") instead of lexically, which would put `10` before `9`. The modelled
/// `Version` token is that integer rendered.
#[derive(Debug, Clone)]
pub struct PromptVersionRecord {
    /// The prompt this version was cut from.
    pub prompt_id: String,
    /// The ARN of this VERSION, as the modelled `PromptArn` spells a qualified one
    /// (`…:prompt/<promptId>:<version>`).
    pub arn: String,
    pub version: u32,
    /// The name the prompt carried when this version was cut.
    pub name: String,
    /// The description of the version: the one the request gave, or the prompt's own when
    /// it gave none.
    pub description: Option<String>,
    pub customer_encryption_key_arn: Option<String>,
    pub default_variant: Option<String>,
    /// The variants the prompt carried when this version was cut, exactly as the draft
    /// held them.
    pub variants: Option<Vec<PromptVariant>>,
    /// The tags the cut attached to this version. A version is addressed by its own
    /// qualified ARN, so its tags are its own and not the draft's.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PromptVersionRecord {
    /// A version cut from `prompt` as version `version` at `now`: the snapshot
    /// `CreatePromptVersion` stores.
    ///
    /// `description` is the request's, which the operation documents as "A description for
    /// the version of the prompt"; a request that gives none leaves the version carrying
    /// the prompt's own description, as the rest of the snapshot does.
    ///
    /// A version is a new, immutable resource minted by this call, so both of its
    /// timestamps are the instant it was cut: `createdAt` is when this version came into
    /// being, and nothing can update it afterwards.
    pub fn snapshot_of(
        prompt: &PromptRecord,
        version: u32,
        description: Option<String>,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            prompt_id: prompt.prompt_id.clone(),
            arn: prompt_version_arn(&prompt.arn, version),
            version,
            name: prompt.name.clone(),
            description: description.or_else(|| prompt.description.clone()),
            customer_encryption_key_arn: prompt.customer_encryption_key_arn.clone(),
            default_variant: prompt.default_variant.clone(),
            variants: prompt.variants.clone(),
            tags,
            created_at: now,
            updated_at: now,
        }
    }

    /// This record as the `CreatePromptVersion` response: every member comes from the
    /// snapshot just stored, never from the request being served.
    pub fn to_create_output(&self) -> CreatePromptVersionOutput {
        CreatePromptVersionOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            default_variant: self.default_variant.clone(),
            variants: self.variants.clone(),
            id: self.prompt_id.clone(),
            arn: self.arn.clone(),
            version: self.version.to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// This record as the `GetPrompt` response for the version it is: the snapshot the
    /// version was cut as, reported from the store rather than read back off the prompt,
    /// so a later change to the draft never moves what a numbered version reports.
    ///
    /// `with_variants` is false for a metadata-only read, which drops the variants and
    /// nothing else.
    pub fn to_get_output(&self, with_variants: bool) -> GetPromptOutput {
        GetPromptOutput {
            name: self.name.clone(),
            description: self.description.clone(),
            customer_encryption_key_arn: self.customer_encryption_key_arn.clone(),
            default_variant: self.default_variant.clone(),
            variants: with_variants.then(|| self.variants.clone()).flatten(),
            id: self.prompt_id.clone(),
            arn: self.arn.clone(),
            version: self.version.to_string(),
            created_at: to_date_time(self.created_at),
            updated_at: to_date_time(self.updated_at),
        }
    }
}

/// The ARN of one numbered version of the prompt `prompt_arn` names: the prompt's own ARN
/// qualified with the version, which is the form the modelled `PromptArn` admits past the
/// prompt id (`…:prompt/<promptId>(?::[0-9]{1,5})?`) and the one a
/// `PromptIdentifier` carrying a version is written in.
pub fn prompt_version_arn(prompt_arn: &str, version: u32) -> String {
    format!("{prompt_arn}:{version}")
}

/// The resource-type field a prompt's ARN carries, as the modelled `PromptArn` spells
/// it (`…:prompt/<promptId>`).
const PROMPT_ARN_RESOURCE_TYPE: &str = "prompt";

/// The modelled `PromptIdentifier` pattern, as the model writes it. A refusal quotes
/// it through [`stated_pattern`], which drops the whole-value anchors.
pub const PROMPT_IDENTIFIER_PATTERN: &str = concat!(
    "^([0-9a-zA-Z]{10})|(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:",
    "prompt/[0-9a-zA-Z]{10})(?::[0-9]{1,5})?$",
);

/// Digits the version qualifier of a `PromptIdentifier` carries, from that pattern's
/// `(?::[0-9]{1,5})?` tail.
const VERSION_QUALIFIER_DIGITS: usize = 5;

/// The `ValidationException` message a prompt operation answers when `promptIdentifier`
/// is none of the forms the modelled shape admits: it names the member and the
/// constraint the value failed, with the value from the request.
pub fn prompt_identifier_pattern_message(identifier: &str) -> String {
    format!(
        "Value '{identifier}' at 'promptIdentifier' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {}",
        stated_pattern(PROMPT_IDENTIFIER_PATTERN)
    )
}

/// What a modelled `PromptIdentifier` names: the prompt, and the version its ARN form
/// may qualify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptSelection<'a> {
    /// The `promptId` the identifier carries, whichever form it took.
    pub prompt_id: &'a str,
    /// The version the identifier qualifies, when it carries the `prompt/<id>:<version>`
    /// tail; `None` when the identifier names the prompt without a version.
    pub version: Option<&'a str>,
}

/// The prompt a modelled `PromptIdentifier` names, or `None` when the value is none of
/// the forms the shape admits.
///
/// `PromptIdentifier` is modelled [`PROMPT_IDENTIFIER_PATTERN`]: either a prompt ARN,
/// whose `prompt/<id>` segment carries the id and which may be qualified with a version,
/// or the bare id. Both forms name the same prompt, so both select the same stored
/// record; anything else names no prompt at all and is a bad request rather than a
/// missing one.
///
/// The six ARN fields are read through `service_common::arn`, the one ARN parse of this
/// workspace; what stays here is this model's own part — the `aws` partition and the
/// Region grammar the pattern states, the account-id rule and the resource field's
/// `prompt/<id>` shape.
pub fn prompt_from_identifier(identifier: &str) -> Option<PromptSelection<'_>> {
    let Some(arn) = ::service_common::arn::parse_of(identifier, ::service_common::bedrock::ARN_NAMESPACE)
    else {
        // Not an ARN of this vendor at all, so the only form left is the bare id.
        return is_minted_id(identifier).then_some(PromptSelection {
            prompt_id: identifier,
            version: None,
        });
    };
    if arn.partition != "aws"
        || ::service_common::arn::region_label(arn.region).is_none()
        || !::service_common::arn::is_account_id(arn.account)
    {
        return None;
    }
    let qualified = arn
        .resource
        .strip_prefix(PROMPT_ARN_RESOURCE_TYPE)?
        .strip_prefix('/')?;
    let (prompt_id, version) = match qualified.split_once(':') {
        None => (qualified, None),
        Some((prompt_id, version)) => (prompt_id, Some(version)),
    };
    if !is_minted_id(prompt_id) || version.is_some_and(|version| !is_version_qualifier(version)) {
        return None;
    }
    Some(PromptSelection { prompt_id, version })
}

/// Whether `version` is the version qualifier a prompt ARN may carry: one to
/// [`VERSION_QUALIFIER_DIGITS`] decimal digits, as the identifier pattern spells it.
/// Whether a version so named EXISTS is a different question, which the lookup answers.
fn is_version_qualifier(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= VERSION_QUALIFIER_DIGITS
        && version.bytes().all(|byte| byte.is_ascii_digit())
}

/// The `ResourceNotFoundException` message a prompt operation answers when the prompt
/// the request names is not stored.
pub fn prompt_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "Prompt")
}

/// The `ResourceNotFoundException` message a prompt operation answers when the VERSION
/// it was asked to resolve is not one the addressed prompt holds.
pub fn prompt_version_not_found_message(operation: &str) -> String {
    resource_not_found_message(operation, "PromptVersion")
}

/// Whether a read carrying this `includedData` is asking for the prompt's VARIANTS.
///
/// The member is documented "Set to METADATA_ONLY to return only resource metadata. Set
/// to ALL_DATA or omit this field to return the full response", and a prompt's variants
/// — the templates, the model and the inference settings it would be invoked with, which
/// the model marks `@sensitive` — are the one non-metadata part of its response. The
/// reading of the member itself is shared with every other read this plane serves
/// ([`included_data_carries_definition`]), so one rule decides what a metadata-only read
/// is.
pub fn included_data_carries_variants(included_data: Option<&str>) -> bool {
    included_data_carries_definition(included_data)
}

/// Whether a prompt's `defaultVariant` names one of the `variants` it carries.
///
/// `defaultVariant` is documented "The name of the default variant for the prompt. This
/// value must match the name field in the relevant PromptVariant object", so a prompt
/// whose default variant names none of its variants is a bad request — a prompt carrying
/// no variants at all included, because then there is no name for it to match. A request
/// that names no default variant has nothing to match and is served.
pub fn default_variant_names_a_variant(
    default_variant: Option<&str>,
    variants: Option<&[PromptVariant]>,
) -> bool {
    let Some(default_variant) = default_variant else {
        return true;
    };
    variants.is_some_and(|variants| {
        variants
            .iter()
            .any(|variant| variant.name == default_variant)
    })
}

/// The `ValidationException` message a prompt operation answers when its
/// `defaultVariant` names none of the variants the prompt carries.
///
/// This pre-condition relates TWO input members to each other rather than measuring one
/// member's value against a constraint of its own, so it is NOT worded through the
/// per-member constraint envelope (`Value '…' at '…' failed to satisfy constraint: …`,
/// which [`prompt_identifier_pattern_message`] and its peers use for a value that fails
/// its own shape's pattern): the service states the cross-member rule as one fixed
/// sentence with a remediation clause and interpolates nothing from the request — not the
/// offending value, not the member name, not the names of the variants supplied.
pub const DEFAULT_VARIANT_NOT_IN_VARIANTS_MESSAGE: &str = concat!(
    "Default variant must be present in the variants list. ",
    "Retry your request with the correct default variant.",
);

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// Mint an unused `promptId`, which the modelled `PromptId` shape constrains to the
    /// same `^[0-9a-zA-Z]{10}$` as an `agentId`. `None` when every one of
    /// `MINT_ATTEMPTS` draws collided.
    pub fn mint_prompt_id(&mut self, now: SystemTime) -> Option<String> {
        for _ in 0..MINT_ATTEMPTS {
            let id = self.next_id_candidate(now);
            if !self.prompts.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// The numbered version the modelled `Version` token `prompt_version` names on this
    /// prompt, or `None` when the prompt holds no version cut under that number.
    ///
    /// A numbered version is identified by the pair, so one prompt never reports another
    /// prompt's version.
    pub fn prompt_version_of(
        &self,
        prompt_id: &str,
        prompt_version: &str,
    ) -> Option<&PromptVersionRecord> {
        let version = numerical_version(prompt_version)?;
        self.prompt_versions.get(&(prompt_id.to_string(), version))
    }

    /// The numbered versions this prompt has been cut into.
    pub fn prompt_version_count(&self, prompt_id: &str) -> usize {
        self.prompt_versions
            .values()
            .filter(|record| record.prompt_id == prompt_id)
            .count()
    }

    /// Forget a prompt, every numbered version cut from it and the indexes that let a
    /// repeated create or cut replay either; `None` when no prompt is stored under that
    /// id, which is the one thing that makes a delete of it a missing resource.
    ///
    /// The prompt's working draft IS this record, so removing it removes the draft, and
    /// the versions cut from it go with it: a numbered version is identified by the
    /// prompt it was cut from, so nothing addresses it once the prompt is gone. The tags
    /// both carry live on those same records, so no tag of a removed prompt survives it.
    pub fn remove_prompt(&mut self, prompt_id: &str) -> Option<PromptRecord> {
        let record = self.prompts.remove(prompt_id)?;
        self.prompt_versions
            .retain(|(versioned_prompt_id, _), _| versioned_prompt_id != prompt_id);
        self.prompt_version_client_tokens
            .retain(|(versioned_prompt_id, _), _| versioned_prompt_id != prompt_id);
        self.prompt_client_tokens.retain(|_, id| id != prompt_id);
        Some(record)
    }

    /// Forget the numbered version the modelled `NumericalVersion` token names on this
    /// prompt, and the index that let a repeated cut replay it; `None` when the prompt
    /// holds no version under that number, which is the one thing that makes a delete of
    /// it a missing resource.
    ///
    /// The prompt's working draft and the versions cut either side of this one are left
    /// exactly as they were, and so is the prompt's numbering:
    /// [`PromptRecord::versions_cut`] is a high-water mark, so the next cut takes the
    /// next number rather than the one the removed version held.
    pub fn remove_prompt_version(
        &mut self,
        prompt_id: &str,
        prompt_version: &str,
    ) -> Option<PromptVersionRecord> {
        let version = numerical_version(prompt_version)?;
        let record = self
            .prompt_versions
            .remove(&(prompt_id.to_string(), version))?;
        self.prompt_version_client_tokens.retain(
            |(versioned_prompt_id, _), cut| versioned_prompt_id != prompt_id || *cut != version,
        );
        Some(record)
    }

    /// The number `CreatePromptVersion` cuts this prompt's next version as: one past the
    /// highest number the prompt has ever reached ([`PromptRecord::versions_cut`]), so a
    /// number a deleted version held is never handed out again, and versions are numbered
    /// incrementally from 1.
    pub fn next_prompt_version(&self, prompt_id: &str) -> u32 {
        self.prompts
            .get(prompt_id)
            .map_or(1, |prompt| prompt.versions_cut.saturating_add(1))
    }
}
