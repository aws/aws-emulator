// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator service.
//!
//! Everything the campaign slice reports is read back off a record stored here, so a
//! create is visible to every later read and a delete is visible as an absence. The
//! module is split by concern: the campaign records and their projections live in
//! [`campaigns`], the AWS-authored template catalog and its search in [`templates`],
//! and the pieces both reach — identity, the clock, the error shapes — here.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use service_common::page_tokens::PageTokenKey;

mod campaigns;
mod templates;

pub use campaigns::*;
pub use templates::*;

/// The service's own ARN namespace, as the Smithy model's `aws.api#service` trait
/// declares it (`arnNamespace` `partnercentral`).
pub const ARN_NAMESPACE: &str = "partnercentral";

/// The service's sigv4 signing name / endpoint prefix, which a quota refusal reports
/// as the service the quota belongs to.
pub const SERVICE_CODE: &str = "partnercentral-marketing";

/// The region used when a request carries no SigV4 credential scope to read one from
/// (an anonymous, host-routed request). Every generated emulator defaults here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `catalog/<Catalog>/campaign/<CampaignId>` resource form of a campaign ARN, as
/// `TaggableResourceArn` documents it.
const CATALOG_SEGMENT: &str = "catalog";
const CAMPAIGN_SEGMENT: &str = "campaign";

/// The id prefix of a campaign (`camp-` + 13 characters of `[A-Za-z0-9]`).
const CAMPAIGN_ID_PREFIX: &str = "camp";

/// The `ResourceType` an error about a campaign reports — the resource's own name in
/// this service's model (`CampaignResource`, which `TaggableResourceArn` references).
pub const CAMPAIGN_RESOURCE_TYPE: &str = "Campaign";

/// The `ResourceType` an error about a campaign template reports.
pub const CAMPAIGN_TEMPLATE_RESOURCE_TYPE: &str = "CampaignTemplate";

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str = "the com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator \
                            store is not mounted on this engine";

/// The largest page a paginated read of this service returns: `MaxResults`' modelled
/// maximum (`range 1..100`) on both `SearchCampaignTemplates` and `ListCampaigns`,
/// which is also the modelled maximum length of either output list.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most campaigns this emulator stores for one catalog.
///
/// A ceiling, not a claim about AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateCampaign` models
/// `ServiceQuotaExceededException`, which is the refusal once the store is full.
pub const MAX_CAMPAIGNS_PER_CATALOG: usize = 500;

/// The most `ClientToken` entries kept for idempotency.
///
/// At most two tokens are recorded per stored campaign (the create's and the last
/// update's) and a campaign's tokens are dropped with it, so this cap is never the
/// binding one in practice; it is enforced anyway, by evicting the oldest token, so the
/// map cannot accumulate caller-chosen strings independently of the campaigns they name.
pub const MAX_CLIENT_TOKENS: usize = 2 * MAX_CAMPAIGNS_PER_CATALOG;

/// Per-service state for com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator. A handler reaches it through [`service_state`].
///
/// The template catalog is NOT here: it is AWS-authored and read-only ([`catalog`]), so
/// every caller reads the same catalog and no read has to write a seed into the store.
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState {
    /// Every campaign, keyed by `Catalog` and then by `CampaignId`. `BTreeMap`s so a
    /// list pages in a stable, resumable order. Each catalog is bounded by
    /// [`MAX_CAMPAIGNS_PER_CATALOG`].
    pub campaigns: BTreeMap<String, BTreeMap<String, CampaignRecord>>,
    /// Idempotency: the key [`idempotency_key`] builds -> the `CampaignId` the first
    /// call with that token resolved to. Bounded by [`MAX_CLIENT_TOKENS`]; write it
    /// through [`Self::record_client_token`].
    pub idempotency: HashMap<String, String>,
    /// The order [`Self::idempotency`] was written in, so the cap evicts the oldest
    /// token rather than an arbitrary one.
    idempotency_order: VecDeque<String>,
    /// The secret this store's `NextToken`s are authenticated under, so a position no
    /// read of this store offered is refused rather than seeked to.
    pub page_token_key: PageTokenKey,
    /// Per-state clock-derived prefix for minted ids, set on the first mint.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state.
    next_id: u64,
}

impl ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState {
    /// The campaigns of `catalog`, or an empty map's worth of nothing when this catalog
    /// holds none.
    pub fn catalog_campaigns(&self, catalog: &str) -> Option<&BTreeMap<String, CampaignRecord>> {
        self.campaigns.get(catalog)
    }

    /// The stored campaign `catalog`/`campaign_id` names, or `None` when it holds none.
    pub fn campaign(&self, catalog: &str, campaign_id: &str) -> Option<&CampaignRecord> {
        self.campaigns.get(catalog)?.get(campaign_id)
    }

    /// The stored campaign `catalog`/`campaign_id` names, for update in place.
    pub fn campaign_mut(
        &mut self,
        catalog: &str,
        campaign_id: &str,
    ) -> Option<&mut CampaignRecord> {
        self.campaigns.get_mut(catalog)?.get_mut(campaign_id)
    }

    /// How many campaigns `catalog` holds — what [`MAX_CAMPAIGNS_PER_CATALOG`] caps.
    pub fn campaign_count(&self, catalog: &str) -> usize {
        self.campaigns.get(catalog).map_or(0, BTreeMap::len)
    }

    /// Store `campaign` under its own catalog and id.
    pub fn insert_campaign(&mut self, campaign: CampaignRecord) {
        self.campaigns
            .entry(campaign.catalog.clone())
            .or_default()
            .insert(campaign.id.clone(), campaign);
    }

    /// Remove the campaign `catalog`/`campaign_id` names — with its tags and its
    /// assets, which are members of the record — and every idempotency token that
    /// resolved to it, so no token outlives the campaign it names. Answers the removed
    /// record, or `None` when the store held none.
    pub fn remove_campaign(
        &mut self,
        catalog: &str,
        campaign_id: &str,
    ) -> Option<CampaignRecord> {
        let removed = self.campaigns.get_mut(catalog)?.remove(campaign_id)?;
        self.forget_client_tokens_for(campaign_id);
        Some(removed)
    }

    /// Mint a fresh, unused `CampaignId`: `camp-` followed by 13 characters of
    /// `[A-Za-z0-9]`, which is what `CampaignId`'s `@pattern` admits.
    ///
    /// The clock-derived seed keeps ids from different runs apart; the counter keeps ids
    /// within one run unique, and the loop re-draws rather than ever handing back an id
    /// already in use in any catalog.
    pub fn mint_campaign_id(&mut self) -> String {
        if self.id_seed == 0 {
            // 20 bits of the current nanosecond clock -> 5 hex digits.
            self.id_seed = ((nanos_since_epoch() as u64) & 0xF_FFFF).max(1);
        }
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = format!(
                "{CAMPAIGN_ID_PREFIX}-{:05x}{:08x}",
                self.id_seed & 0xF_FFFF,
                self.next_id & 0xFFFF_FFFF
            );
            if !self
                .campaigns
                .values()
                .any(|campaigns| campaigns.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The `CampaignId` a previous call with this idempotency `key` resolved to.
    pub fn client_token_campaign(&self, key: &str) -> Option<&String> {
        self.idempotency.get(key)
    }

    /// Record `key` as having resolved to `campaign_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_CLIENT_TOKENS`].
    pub fn record_client_token(&mut self, key: String, campaign_id: &str) {
        // Re-recording a key already in the map replaces its value rather than growing
        // it, so no eviction is owed and an unrelated live entry is kept.
        if let Some(recorded) = self.idempotency.get_mut(&key) {
            *recorded = campaign_id.to_string();
            return;
        }
        while self.idempotency.len() >= MAX_CLIENT_TOKENS {
            match self.idempotency_order.pop_front() {
                Some(oldest) => {
                    self.idempotency.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap an
                // invariant even if they ever drift.
                None => {
                    self.idempotency.clear();
                    break;
                }
            }
        }
        if self
            .idempotency
            .insert(key.clone(), campaign_id.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
    }

    /// Drop every idempotency token that resolved to `campaign_id`.
    fn forget_client_tokens_for(&mut self, campaign_id: &str) {
        self.idempotency.retain(|_, recorded| recorded != campaign_id);
        let live = &self.idempotency;
        self.idempotency_order
            .retain(|key| live.contains_key(key));
    }
}

/// The key an idempotency token is recorded under.
///
/// `Catalog` and the operation are part of it because a `ClientToken` is the caller's
/// own string: the same token sent to another catalog, or to another operation, names
/// another request and must not resolve to the first one's campaign.
pub fn idempotency_key(catalog: &str, operation: &str, client_token: &str) -> String {
    format!("{catalog}\u{1f}{operation}\u{1f}{client_token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState>)
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// How many SUBSECOND digits a timestamp this service reports carries.
///
/// The model types every audit member as `Timestamp` and says nothing about precision,
/// so this is the resolution the emulator reports one at: millisecond, the resolution
/// this service's wire protocol (`rpcv2Cbor`) encodes an epoch date-time item at.
const SUBSECOND_DIGITS: u32 = 3;

/// A stored instant as the modelled `Timestamp` the wire carries, at the resolution
/// this service reports one at ([`SUBSECOND_DIGITS`]).
///
/// The ONE encoder for every timestamp this service reports, so no shape's projection
/// can report an instant in a different form from another's. Truncated rather than
/// rounded, so the reported instant never moves ahead of the one the clock read.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let scale = 10_u32.pow(9 - SUBSECOND_DIGITS);
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        (since_epoch.subsec_nanos() / scale) * scale,
    )
}

/// The timestamp format this service reads and writes a date through: the protocol's
/// own RFC 3339 rendering, which is what makes the calendar conversion below the date
/// type's rather than this module's.
const DATE_TIME_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// `instant`'s UTC calendar date as the `YYYY-MM-DD` a `UtcDate` carries — the date
/// type renders the instant and this takes the date part — or `None` when the instant
/// has no RFC 3339 rendering at all, which is every instant outside years `0001..=9999`.
///
/// Fallible rather than asserted: the one caller asks about the emulator's own clock,
/// and answers the modelled server error rather than guessing a date.
pub fn utc_date(at: SystemTime) -> Option<String> {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(DATE_TIME_FORMAT)
        .ok()?;
    Some(rendered.chars().take(10).collect())
}

/// The instant an RFC 3339 `date-time` text names, or `None` when it names none.
///
/// Total over arbitrary text: the calendar (month lengths, the leap rule) is the date
/// type's, and anything it refuses answers `None` for the caller to leave the member
/// absent rather than unwinding.
pub fn parse_instant(value: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(value, DATE_TIME_FORMAT).ok()
}

/// The ARN of the campaign `campaign_id` in `catalog`, as `TaggableResourceArn`
/// documents the form: `arn:aws:partnercentral:<region>:<account>:catalog/<Catalog>/campaign/<CampaignId>`.
pub fn campaign_arn(region: &str, catalog: &str, campaign_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CATALOG_SEGMENT}/{catalog}/{CAMPAIGN_SEGMENT}/{campaign_id}"),
    )
}

/// The `CampaignId` a `CampaignIdentifier` names in `catalog`, or `None` when it names
/// no campaign of this catalog.
///
/// A `CampaignIdentifier` is "a campaign, named by its id or its ARN", so both forms
/// resolve to the same id. An ARN naming another account, another catalog or another
/// service's resource names no campaign here — which is what lets a read refuse it as
/// absent and a list leave it out, as `ListCampaigns$CampaignIdentifiers` documents
/// ("a campaign ... whose ARN names another account or catalog is left out rather than
/// refused").
pub fn campaign_id_of(catalog: &str, identifier: &str) -> Option<String> {
    if !identifier.starts_with("arn:") {
        return is_campaign_id(identifier).then(|| identifier.to_string());
    }
    let arn = ::service_common::arn::parse(identifier)?;
    if arn.vendor != ARN_NAMESPACE || arn.account != ::engine::EMULATOR_ACCOUNT_ID {
        return None;
    }
    let mut segments = arn.resource.split('/');
    if segments.next()? != CATALOG_SEGMENT || segments.next()? != catalog {
        return None;
    }
    if segments.next()? != CAMPAIGN_SEGMENT {
        return None;
    }
    let campaign_id = segments.next()?;
    if segments.next().is_some() || !is_campaign_id(campaign_id) {
        return None;
    }
    Some(campaign_id.to_string())
}

/// Whether `value` has the form of a `CampaignId`: the prefix this service mints one
/// under, then 13 characters of `[A-Za-z0-9]`.
fn is_campaign_id(value: &str) -> bool {
    let Some(suffix) = value
        .strip_prefix(CAMPAIGN_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix.len() == CAMPAIGN_ID_SUFFIX_CHARS
        && suffix.chars().all(|character| character.is_ascii_alphanumeric())
}

/// How many characters follow the `camp-` prefix of a `CampaignId`, from its
/// `@pattern` (`^camp-[A-Za-z0-9]{13}$`).
const CAMPAIGN_ID_SUFFIX_CHARS: usize = 13;

/// The page size a paginated read serves: what the request asked for, clamped into the
/// modelled `MaxResults` range, and [`MAX_PAGE_SIZE`] when it asked for nothing.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE)
}

/// Where a paginated read resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePoint {
    /// Resume at this offset into the sequence — `0` when the request carried no
    /// `NextToken`.
    At(usize),
    /// The request carried a `NextToken` this read did not issue — one no store of this
    /// emulator minted, or one minted for another sequence — so there is no position to
    /// resume at and the read refuses it.
    NotIssued,
}

/// The separator between a cursor's two parts. A unit separator cannot occur in either,
/// so the split is unambiguous.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// Where `next_token` resumes a read of the sequence `scope` names.
///
/// The token is authenticated under this store's own key, so a position no read offered
/// is refused rather than seeked to, and it carries the identity of the sequence it was
/// issued for, so a token one set of filters produced is not read as a position in
/// another.
pub fn resume_point(key: &PageTokenKey, scope: &str, next_token: Option<&String>) -> ResumePoint {
    let Some(next_token) = next_token else {
        return ResumePoint::At(0);
    };
    let Some(cursor) = ::service_common::page_tokens::page_cursor(key, next_token) else {
        return ResumePoint::NotIssued;
    };
    let Some((issued_for, offset)) = cursor.split_once(CURSOR_SEPARATOR) else {
        return ResumePoint::NotIssued;
    };
    if issued_for != ::service_common::page_tokens::scope_digest(scope) {
        return ResumePoint::NotIssued;
    }
    match offset.parse::<usize>() {
        Ok(offset) => ResumePoint::At(offset),
        Err(_) => ResumePoint::NotIssued,
    }
}

/// The `NextToken` a read of the sequence `scope` names hands back to resume at
/// `offset`.
pub fn continuation_token(key: &PageTokenKey, scope: &str, offset: usize) -> String {
    ::service_common::page_tokens::page_token(
        key,
        &format!(
            "{}{CURSOR_SEPARATOR}{offset}",
            ::service_common::page_tokens::scope_digest(scope)
        ),
    )
}

/// The server error for the one failure that is not about the request.
pub fn no_store() -> crate::ops::types::InternalServerException {
    crate::ops::types::InternalServerException {
        message: NO_STORE.to_string(),
        retry_after_seconds: None,
    }
}

/// The server error for a clock this service cannot place on the calendar, which is
/// what a state derivation and a date comparison both need.
pub fn clock_unavailable() -> crate::ops::types::InternalServerException {
    crate::ops::types::InternalServerException {
        message: "the emulator's clock does not fall on a UTC calendar date".to_string(),
        retry_after_seconds: None,
    }
}

/// `ResourceNotFoundException` for a `CampaignIdentifier` that names no stored
/// campaign — one of a form this service does not mint, one naming another catalog or
/// account, one that was deleted, and one that was never minted.
///
/// The identifier is echoed from the request, which is all that varies.
pub fn campaign_not_found(identifier: &str) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: format!("Campaign {identifier} does not exist."),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(CAMPAIGN_RESOURCE_TYPE.to_string()),
    }
}

/// `ResourceNotFoundException` for a `CampaignTemplateId` the catalog does not hold —
/// including a template whose `EffectiveDateBegin` has not been reached, which "does
/// not exist to a partner" until then.
pub fn campaign_template_not_found(
    identifier: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: format!("Campaign template {identifier} does not exist."),
        resource_id: Some(identifier.to_string()),
        resource_type: Some(CAMPAIGN_TEMPLATE_RESOURCE_TYPE.to_string()),
    }
}

/// `ServiceQuotaExceededException` for a catalog already holding
/// [`MAX_CAMPAIGNS_PER_CATALOG`] campaigns.
pub fn campaign_quota_exceeded(
    catalog: &str,
) -> crate::ops::types::ServiceQuotaExceededException {
    crate::ops::types::ServiceQuotaExceededException {
        message: format!(
            "Catalog {catalog} already holds the maximum of {MAX_CAMPAIGNS_PER_CATALOG} campaigns."
        ),
        resource_type: Some(CAMPAIGN_RESOURCE_TYPE.to_string()),
        // No `QuotaCode`: a Service Quotas code is AWS's own identifier for the quota
        // and this ceiling is the emulator's, so none is reported rather than invented.
        quota_code: None,
        service_code: Some(SERVICE_CODE.to_string()),
    }
}

/// `ValidationException` for a request whose own members failed, with every failing
/// member named in `FieldList`.
///
/// `FieldList` carries "every field that failed, not just the first", and `Reason`
/// says so coarsely for a caller that does not read the list.
pub fn field_validation_failed(
    fields: Vec<crate::ops::types::ValidationExceptionField>,
) -> crate::ops::types::ValidationException {
    let names: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    crate::ops::types::ValidationException {
        message: format!("The following fields failed validation: {}.", names.join(", ")),
        reason: Some(VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string()),
        field_list: Some(fields),
    }
}

/// One failing member of the request, as `ValidationExceptionField` reports it: the
/// member's own name, what is wrong with it, and the machine-readable code.
pub fn invalid_value_field(
    name: &str,
    message: String,
) -> crate::ops::types::ValidationExceptionField {
    crate::ops::types::ValidationExceptionField {
        name: name.to_string(),
        // No `FieldId`: that names a template FIELD inside `FieldValues`, and these
        // entries name campaign settings, which are members of the request itself.
        field_id: None,
        message,
        code: Some(VALIDATION_FIELD_CODE_INVALID_VALUE.to_string()),
    }
}

/// `ValidationException` for a refusal that points at nothing the caller fixes in a
/// form — a tag key, a continuation token — which carries `Message` only.
pub fn invalid_request(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message,
        reason: Some(VALIDATION_REASON_OTHER.to_string()),
        field_list: None,
    }
}

/// `ValidationException` for a `NextToken` this read did not issue: one no store of
/// this emulator minted, and one minted for another set of filters or another sort
/// order, which `ListCampaigns$NextToken` documents as refused.
pub fn invalid_next_token() -> crate::ops::types::ValidationException {
    invalid_request(
        "The NextToken is not a continuation token this operation issued for this \
         request's filters and sort order."
            .to_string(),
    )
}

/// `ConflictException` for an update of a campaign whose run window has closed:
/// `UpdateCampaign` is "refused ... once the campaign is completed
/// (`CAMPAIGN_COMPLETED`, the UTC date is after its `EndOn`)".
pub fn campaign_completed(
    campaign_id: &str,
    end_on: &str,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "Campaign {campaign_id} completed after its EndOn of {end_on} and cannot be changed."
        ),
        resource_id: Some(campaign_id.to_string()),
        resource_type: Some(CAMPAIGN_RESOURCE_TYPE.to_string()),
        reason: Some(CONFLICT_REASON_CAMPAIGN_COMPLETED.to_string()),
    }
}

/// `ConflictException` for a delete of a campaign that still has a live asset:
/// `DeleteCampaign` is "refused with ConflictException while the campaign has a live
/// asset (`CAMPAIGN_HAS_ACTIVE_ASSETS`) ... deactivate those assets first".
pub fn campaign_has_active_assets(campaign_id: &str) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "Campaign {campaign_id} has active assets. Deactivate them before deleting it."
        ),
        resource_id: Some(campaign_id.to_string()),
        resource_type: Some(CAMPAIGN_RESOURCE_TYPE.to_string()),
        reason: Some(CONFLICT_REASON_CAMPAIGN_HAS_ACTIVE_ASSETS.to_string()),
    }
}

/// Every `Tags` rule `CreateCampaign$Tags` states, as the refusal for the first list
/// that breaks one, or `None` when the list breaks none.
///
/// "A key beginning `aws:` (any case) is reserved for AWS-applied system tags and is
/// refused with `ValidationException`, as is a key repeated in the list; nothing is
/// created." A tag refusal "carries `Message` only".
pub fn tag_refusal(
    tags: &[crate::ops::types::Tag],
) -> Option<crate::ops::types::ValidationException> {
    let mut seen: Vec<&str> = Vec::with_capacity(tags.len());
    for tag in tags {
        if tag.key.to_lowercase().starts_with(RESERVED_TAG_KEY_PREFIX) {
            return Some(invalid_request(format!(
                "The tag key {} is reserved for AWS use: a key beginning with \"{}\" cannot be \
                 applied.",
                tag.key, RESERVED_TAG_KEY_PREFIX
            )));
        }
        if seen.contains(&tag.key.as_str()) {
            return Some(invalid_request(format!(
                "The tag key {} appears more than once in Tags.",
                tag.key
            )));
        }
        seen.push(tag.key.as_str());
    }
    None
}

/// Every run-window rule `UtcDate`'s two members state, as one entry per member that
/// failed — so a request breaking both is told about both, which is what `FieldList`
/// carrying "every field that failed, not just the first" asks for.
///
/// `EndOn` is "not in the past" (it is the campaign's last day, inclusive, so today's
/// date still passes), and `StartOn` is "not after `EndOn`". Both read the values the
/// campaign WOULD have, so an update that moves one of them is judged against the other
/// as stored.
pub fn run_window_fields(
    start_on: Option<&str>,
    end_on: Option<&str>,
    today: &str,
) -> Vec<crate::ops::types::ValidationExceptionField> {
    let mut fields = Vec::new();
    if let Some(end_on) = end_on {
        if end_on < today {
            fields.push(invalid_value_field(
                "EndOn",
                format!(
                    "EndOn {end_on} is in the past: a campaign's last day must be {today} or \
                     later."
                ),
            ));
        }
    }
    if let (Some(start_on), Some(end_on)) = (start_on, end_on) {
        if start_on > end_on {
            fields.push(invalid_value_field(
                "StartOn",
                format!("StartOn {start_on} must not be after EndOn {end_on}."),
            ));
        }
    }
    fields
}

/// The tag-key prefix AWS reserves for its own system tags, compared without case.
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// `ConflictException$Reason` for a campaign whose run window has closed — one of the
/// `SCREAMING_SNAKE_CASE` tokens that member documents, as `UpdateCampaign` names it.
const CONFLICT_REASON_CAMPAIGN_COMPLETED: &str = "CAMPAIGN_COMPLETED";

/// `ConflictException$Reason` for a campaign that still has a live asset, as
/// `DeleteCampaign` names it.
const CONFLICT_REASON_CAMPAIGN_HAS_ACTIVE_ASSETS: &str = "CAMPAIGN_HAS_ACTIVE_ASSETS";

/// `ValidationExceptionReason` for a request whose named members failed — a member of
/// [`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`].
const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationExceptionReason` for a request that is invalid for a reason no single
/// member is attributable for.
const VALIDATION_REASON_OTHER: &str = "OTHER";

/// `ValidationExceptionFieldCode` for a member whose value this service will not accept
/// — a member of [`crate::ops::types::VALIDATION_EXCEPTION_FIELD_CODE_VALUES`].
const VALIDATION_FIELD_CODE_INVALID_VALUE: &str = "INVALID_VALUE";

/// The current nanosecond clock, as the seed a minted id draws its prefix from.
fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos())
}
