// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcases-amazonconnectcases service.
//!
//! One `Domain` is the container for everything Cases persists: its fields,
//! templates, layouts and cases all hang off the domain, which is how the real service
//! scopes them (every operation but `CreateDomain`/`ListDomains` carries a
//! `domainId` path label). Handlers reach this store through [`service_state`]
//! and never build their own engine.
//!
//! # Bounding request-driven growth
//!
//! Every map here grows one entry per request, with caller-chosen contents, so
//! each one carries an explicit ceiling: [`MAX_DOMAINS`],
//! [`MAX_FIELDS_PER_DOMAIN`], [`MAX_TEMPLATES_PER_DOMAIN`],
//! [`MAX_LAYOUTS_PER_DOMAIN`], [`MAX_CASES_PER_DOMAIN`],
//! [`MAX_RELATED_ITEMS_PER_CASE`], [`MAX_AUDIT_EVENTS_PER_CASE`] and
//! [`MAX_IDEMPOTENCY_TOKENS`]. Creating past a
//! ceiling is refused with a modelled error, and the idempotency table evicts
//! its oldest entry rather than growing, so no caller can grow this store
//! without end.
//!
//! The per-request work is bounded the same way, and does not rest on the
//! boundary validator alone: [`MAX_CASE_FIELD_IDS`],
//! [`MAX_BATCH_FIELD_IDS`], [`MAX_CUSTOM_RELATED_ITEM_FIELDS`],
//! [`MAX_EVENT_INCLUDED_FIELD_IDS`], [`MAX_SEARCH_CASE_FIELD_IDS`],
//! [`MAX_SEARCH_SORTS`] and [`MAX_CASE_FILTER_CLAUSES`] are re-checked
//! in the handler ([`list_too_long`]),
//! the scans below are linear in the list length, and a refusal names at most
//! [`MAX_IDS_NAMED_IN_A_REFUSAL`] ids. A list that carries the same id twice is
//! refused on the WRITE paths only ([`duplicate_ids`]), where two values for one
//! id is ambiguous; a read answers one entry per occurrence, since the model
//! declares no distinctness constraint and the caps already bound the response.
//!
//! A listing's answer is bounded the same way: [`page`] cuts it at
//! [`MAX_PAGE_SIZE`] even when the request names no `maxResults`, and hands back
//! an opaque `nextToken` for the rest, so no single response grows with the
//! store.

// Comparing a stored field value against one of the model's `FieldFilter`
// comparisons: what every search in this service narrows with.
mod field_filters;
// Projecting and narrowing a case's related items: what every related-item
// reader reports.
mod related_items;

pub use field_filters::*;
pub use related_items::*;

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BasicLayout, ConflictException, EventBridgeConfiguration, FieldAttributes, FieldGroup,
    FieldOption, FieldValueUnion, InternalServerException, LayoutConfiguration, LayoutContent,
    LayoutSections, RelatedItemInputContent, RequiredField, ResourceNotFoundException, Section,
    ServiceQuotaExceededException, TagPropagationConfiguration, UserUnion, ValidationException,
};

/// The ARN namespace (and SigV4 signing name / endpoint prefix) of this service,
/// as `OPERATIONS.md` records it from the model: `cases`.
pub const ARN_SERVICE: &str = "cases";

/// Region used in an ARN when the request carries no SigV4 credential scope to
/// read one from. The emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `DomainStatus` value a domain that exists and serves requests reports.
/// One of `DOMAIN_STATUS_VALUES` (`Active | CreationFailed | CreationInProgress`).
pub const DOMAIN_STATUS_ACTIVE: &str = "Active";

/// `TemplateStatus` a template takes when `CreateTemplate` is given no `status`.
/// One of `TEMPLATE_STATUS_VALUES` (`Active | Inactive`).
pub const TEMPLATE_STATUS_ACTIVE: &str = "Active";

/// The one `FieldType` that has field options: the model documents
/// `BatchPutFieldOptions` as creating and updating "a set of field options for a
/// single select field", so options belong to a field of this type and to no
/// other. One of `FIELD_TYPE_VALUES`.
pub const FIELD_TYPE_SINGLE_SELECT: &str = "SingleSelect";

/// `FieldNamespace` of a field a customer created through `CreateField`; the
/// service's own built-in fields are the other value, `System`.
/// One of `FIELD_NAMESPACE_VALUES` (`Custom | System`).
pub const FIELD_NAMESPACE_CUSTOM: &str = "Custom";

/// `FieldNamespace` of a field the SERVICE defines, as
/// [`RESERVED_SYSTEM_FIELDS`] seeds them into every new domain.
/// One of `FIELD_NAMESPACE_VALUES` (`Custom | System`).
pub const FIELD_NAMESPACE_SYSTEM: &str = "System";

/// The reserved `System`-namespace fields every Cases domain is provisioned
/// with, as `(fieldId, name, FieldType)`. A domain's schema holds these from the
/// moment it is created — no `CreateField` call makes them — so a case can be
/// written against them straight away.
///
/// The model does NOT enumerate them: `FieldId` is a free-form string
/// (`smithy_query(query="shape",shape="FieldId")`, `@length 1..500`) and
/// `FieldNamespace` only names the two namespaces
/// (`smithy_query(query="shape",shape="FieldNamespace")` = `Custom | System`).
/// The ids, display names and types below are the service's own reserved
/// vocabulary, as the Amazon Connect Administrator Guide's "Cases fields" tables
/// list them and as the recorded traffic shows them: the ids are reserved words,
/// verbatim, not minted per domain, and each one's `FieldType` is a value of
/// `FIELD_TYPE_VALUES` so a value written against it is type-checked like any
/// other field's.
///
/// The second column is the field's NAME, which the model keeps distinct from
/// its id: `FieldSummary.name` is its own required member, "Name of the field"
/// (`smithy_query(query="shape",shape="FieldSummary")`), typed `FieldName`
/// (`@length 1..100`, pattern `^.*[\S]$`). It is the service's display name for
/// the field — the guide's own wording for it, which is how a client that only
/// knows the display name resolves the id it has to send — so it is not the
/// reserved word the id is.
///
/// Two of them — `created_datetime` and `last_updated_datetime` — are instants
/// the real service maintains itself. Neither the model nor the recorded traffic
/// says whether it refuses, ignores or accepts a caller's write to them, so they
/// are provisioned as the ordinary `DateTime` fields they are typed as and the
/// question is left to a recording that settles it.
pub const RESERVED_SYSTEM_FIELDS: &[(&str, &str, &str)] = &[
    ("assigned_queue", "Assigned queue", "Text"),
    ("assigned_user", "Assigned user", "User"),
    ("case_reason", "Case reason", "SingleSelect"),
    ("created_datetime", "Date/time created", "DateTime"),
    ("customer_id", "Customer", "Text"),
    ("last_updated_datetime", "Date/time last updated", "DateTime"),
    ("reference_number", "Reference number", "Text"),
    ("status", "Status", "Text"),
    ("summary", "Summary", "Text"),
    ("title", "Title", "Text"),
];

/// Most domains this emulator's store holds. EMULATOR-OWNED ceiling on
/// request-driven growth, not an AWS-published quota: the real service's limit
/// is an account quota a support request can raise, while this one exists so a
/// caller cannot grow the in-memory store without end.
pub const MAX_DOMAINS: usize = 100;

/// Most fields one domain holds for a caller. Emulator-owned ceiling, as
/// [`MAX_DOMAINS`]. It also bounds a case's stored field values, since only a
/// field the domain defines can be written — the reserved
/// [`RESERVED_SYSTEM_FIELDS`] beside it are a fixed, service-provisioned set.
pub const MAX_FIELDS_PER_DOMAIN: usize = 500;

/// Most templates one domain holds. Emulator-owned ceiling, as [`MAX_DOMAINS`].
pub const MAX_TEMPLATES_PER_DOMAIN: usize = 500;

/// Most layouts one domain holds. This one is the service's OWN published
/// figure, which `DeleteLayout`'s documentation restates — "You can delete up to
/// 100 layouts per domain"
/// (`smithy_query(query="doc", shape="DeleteLayout")`) — and it doubles as this
/// store's ceiling on request-driven growth, as [`MAX_DOMAINS`].
pub const MAX_LAYOUTS_PER_DOMAIN: usize = 100;

/// Most sections one layout panel (`topPanel` / `moreInfo`) may carry: the
/// model's `SectionsList` cap of `..1`
/// (`smithy_query(query="shape", shape="SectionsList")`), re-checked in the
/// handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_LAYOUT_SECTIONS: usize = 1;

/// Most field items one layout field group may list: the model's
/// `FieldGroup$fields` cap of `..220`
/// (`smithy_query(query="shape", shape="FieldGroup")`), re-checked in the
/// handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_LAYOUT_SECTION_FIELDS: usize = 220;

/// Most cases one domain holds. Emulator-owned ceiling, as [`MAX_DOMAINS`].
pub const MAX_CASES_PER_DOMAIN: usize = 10_000;

/// Most related items one case holds. Emulator-owned ceiling on request-driven
/// growth, as [`MAX_CASES_PER_DOMAIN`]: every accepted `CreateRelatedItem`
/// stores one more item under its case, with caller-chosen content, so past
/// this many a further association is refused with the modelled quota error
/// rather than the store growing with the number of associations one caller
/// makes.
pub const MAX_RELATED_ITEMS_PER_CASE: usize = 1_000;

/// Most `CreateCase` idempotency tokens the store remembers. Emulator-owned
/// ceiling, as [`MAX_DOMAINS`]: past it the oldest remembered token is
/// forgotten, which is how a real idempotency window behaves — a replay after
/// the window mints a new resource rather than being refused.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// Most field ids one case request may carry. This is the model's own cap,
/// declared as a member-level `length` trait rather than on the list shape:
/// `GetCaseRequest$fields` is `1..220` and `CreateCaseRequest$fields` /
/// `UpdateCaseRequest$fields` are `..220`
/// (`smithy_query(query="shape", shape="GetCaseRequest,CreateCaseRequest,UpdateCaseRequest")`).
/// It is restated here and re-checked inside the handler by [`list_too_long`],
/// so the bound on caller-chosen per-request work does not rest on the boundary
/// validator alone.
pub const MAX_CASE_FIELD_IDS: usize = 220;

/// Most field ids one `BatchGetField` request may carry: the model's
/// `BatchGetFieldRequest$fields` cap of `1..50`, re-checked in the handler for
/// the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_BATCH_FIELD_IDS: usize = 50;

/// Most field values one `Custom`-type related item may carry: the model's
/// `CustomInputContent$fields` cap of `1..50`, which is the quota
/// `CreateRelatedItem` documents ("There's a quota for the number of fields
/// allowed in a Custom type related item"), re-checked in the handler for the
/// same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_CUSTOM_RELATED_ITEM_FIELDS: usize = 50;

/// Most field options one `BatchPutFieldOptions` request may carry: the model's
/// `BatchPutFieldOptionsRequest$options` cap of `..50`, re-checked in the
/// handler for the same reason as [`MAX_CASE_FIELD_IDS`]. It also bounds the
/// response's `errors` list, whose own cap is the same `..50`.
pub const MAX_BATCH_FIELD_OPTIONS: usize = 50;

/// Most field options one single-select field holds. Emulator-owned ceiling on
/// request-driven growth, as [`MAX_DOMAINS`]: the real service's limit is an
/// account quota, while this one exists so a caller cannot grow one field's
/// option set without end. Putting a NEW option past it is refused with the
/// modelled quota error; updating one the field already holds is not growth and
/// is always accepted.
pub const MAX_FIELD_OPTIONS_PER_FIELD: usize = 500;

/// Most values one `ListFieldOptions` request may filter on: the model's
/// `ListFieldOptionsRequest$values` cap of `..1`, re-checked in the handler for
/// the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_LIST_FIELD_OPTION_VALUES: usize = 1;

/// Most required fields one template may declare: the model's
/// `CreateTemplateRequest$requiredFields` cap of `..100`, re-checked in the
/// handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_REQUIRED_FIELDS: usize = 100;

/// Most field identifiers one domain's case-event publishing configuration may
/// include: the model's `CaseEventIncludedData$fields` cap of `..400`
/// (`smithy_query(query="shape", shape="CaseEventIncludedData")`), re-checked in
/// the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_EVENT_INCLUDED_FIELD_IDS: usize = 400;

/// Most field ids one `SearchCases` request may project: the model's
/// `SearchCasesRequest$fields` cap of `0..25`, which is also the cap on the
/// `SearchCasesResponseItem$fields` list each answer carries
/// (`smithy_query(query="operation", operation="SearchCases")`). Re-checked in
/// the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_SEARCH_CASE_FIELD_IDS: usize = 25;

/// Most sort terms one search of this service may apply: the cap the model
/// states for BOTH search sort lists, `SearchCasesRequest$sorts` and
/// `SearchAllRelatedItemsRequest$sorts`, which is `0..2` for each
/// (`smithy_query(query="operation", operation="SearchCases,SearchAllRelatedItems")`),
/// re-checked in the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_SEARCH_SORTS: usize = 2;

/// Entries one `CaseFilter` list (`andAll` / `orAll`) may carry: the model's
/// `CaseFilterList` cap of `..10`
/// (`smithy_query(query="shape", shape="CaseFilterList")`).
pub const MAX_CASE_FILTER_CLAUSES_PER_LIST: usize = 10;

/// Most clauses ONE `CaseFilter` tree may hold in all. `CaseFilter` nests
/// through `not`, `andAll` and `orAll` without a modelled bound, so a tree of
/// `MAX_CASE_FILTER_CLAUSES_PER_LIST`-wide lists grows its clause count
/// multiplicatively with its depth; the search evaluates every clause against
/// every candidate case, so the total is bounded here as well, keeping the work
/// one request can ask for linear in this constant.
pub const MAX_CASE_FILTER_CLAUSES: usize = 100;

/// Deepest `CaseFilter` nesting the search evaluates, bounding the recursion a
/// single request can drive for the same reason as
/// [`MAX_CASE_FILTER_CLAUSES`].
pub const MAX_CASE_FILTER_DEPTH: usize = 10;

/// Entries one `SearchCases` answer carries when the request names no
/// `maxResults`: the model documents "When no value is provided, 25 is the
/// default"
/// (`smithy_query(query="doc", shape="SearchCasesRequest", member="maxResults")`),
/// which is this listing's own default rather than the service-wide
/// [`MAX_PAGE_SIZE`].
pub const DEFAULT_SEARCH_CASES_PAGE_SIZE: i32 = 25;

/// Most ids a refusal message names, so a refusal over a long list cannot be
/// amplified into a long response.
pub const MAX_IDS_NAMED_IN_A_REFUSAL: usize = 10;

/// Most audit-history events one case keeps. Emulator-owned ceiling on
/// request-driven growth, as [`MAX_IDEMPOTENCY_TOKENS`]: every accepted edit of
/// a case appends one event to that case's history, so past this many the
/// OLDEST event is dropped rather than the store growing with the number of
/// edits a caller makes ([`Case::record_audit_event`]).
pub const MAX_AUDIT_EVENTS_PER_CASE: usize = 100;

/// Most audit events one page of a case's audit history carries, which is also
/// the page size a request naming no `maxResults` gets. The model's own cap,
/// stated twice: `GetCaseAuditEventsRequest$maxResults` is `@range(1..25)` and
/// `GetCaseAuditEventsResponse$auditEvents` is `@length(max: 25)`
/// (`smithy_query(query="operation", operation="GetCaseAuditEvents")`), and the
/// model documents the same number as the default — "The maximum number of
/// audit events to return. When no value is provided, 25 is the default."
pub const MAX_AUDIT_EVENTS_PER_PAGE: i32 = 25;

/// `AuditEventType` of the event a case's creation records.
/// One of `AUDIT_EVENT_TYPE_VALUES`.
pub const AUDIT_EVENT_CASE_CREATED: &str = "Case.Created";

/// `AuditEventType` of the event an edit of a case's field values records.
/// One of `AUDIT_EVENT_TYPE_VALUES`.
pub const AUDIT_EVENT_CASE_UPDATED: &str = "Case.Updated";

/// `AuditEventType` of the event the association of a related item with a case
/// records. One of `AUDIT_EVENT_TYPE_VALUES`.
pub const AUDIT_EVENT_RELATED_ITEM_CREATED: &str = "RelatedItem.Created";

/// A Cases domain and everything scoped to it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub domain_arn: String,
    pub name: String,
    pub created_time: SystemTime,
    pub domain_status: String,
    pub tags: HashMap<String, String>,
    pub fields: HashMap<String, Field>,
    pub templates: HashMap<String, Template>,
    pub layouts: HashMap<String, Layout>,
    pub cases: HashMap<String, Case>,
    pub event_bridge: Option<EventBridgeConfiguration>,
}

/// A case field definition: the schema a case's field values are written against.
#[derive(Debug, Clone)]
pub struct Field {
    pub field_id: String,
    pub field_arn: String,
    pub name: String,
    /// One of `FIELD_TYPE_VALUES`.
    pub field_type: String,
    /// One of `FIELD_NAMESPACE_VALUES`.
    pub namespace: String,
    pub description: Option<String>,
    pub attributes: Option<FieldAttributes>,
    /// The field's options, keyed by `FieldOption$value` — the value identifies
    /// the option within its field, which is why `BatchPutFieldOptions` is an
    /// upsert on it. Ordered by value (a `BTreeMap`), so `ListFieldOptions`
    /// answers in one stable order and a page cursor naming the last value of a
    /// page resumes correctly even when options are added between pages.
    /// Bounded by [`MAX_FIELD_OPTIONS_PER_FIELD`]. Only a
    /// [`FIELD_TYPE_SINGLE_SELECT`] field can carry any.
    pub options: BTreeMap<String, FieldOption>,
    pub deleted: bool,
    /// Reserved for the tagging slice: `CreateField` takes no tags and
    /// `TagResource`/`UntagResource` are still 501s, so nothing can populate
    /// this yet and a read reports no `tags` member for a field.
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A case template.
#[derive(Debug, Clone)]
pub struct Template {
    pub template_id: String,
    pub template_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub layout_configuration: Option<LayoutConfiguration>,
    pub required_fields: Vec<RequiredField>,
    pub tag_propagation_configurations: Vec<TagPropagationConfiguration>,
    /// One of `TEMPLATE_STATUS_VALUES`.
    pub status: String,
    /// Whether `DeleteTemplate` has deleted this template. The service's delete
    /// is a SOFT delete, so the record carries a flag rather than being
    /// dropped: a deleted template is still readable through `GetTemplate`,
    /// refuses `UpdateTemplate`, cannot be used to create a case, and is left
    /// out of `ListTemplates`.
    pub deleted: bool,
    /// Reserved for the tagging slice, as [`Field::tags`].
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A case layout: which of the domain's fields the Cases user interface shows,
/// and in what order.
#[derive(Debug, Clone)]
pub struct Layout {
    pub layout_id: String,
    pub layout_arn: String,
    pub name: String,
    /// The content as it was last written, in the complete form
    /// [`normalized_layout_content`] keeps it in: every section, each field
    /// group's name and the order of its fields are exactly what the write
    /// gave, so a read hands back the record rather than a re-encoding of it.
    pub content: LayoutContent,
    /// Whether `DeleteLayout` has deleted this layout. The service's delete is
    /// a SOFT delete, so the record carries a flag rather than being dropped: a
    /// deleted layout is still readable through `GetLayout`, refuses
    /// `UpdateLayout`, and is left out of `ListLayouts`.
    pub deleted: bool,
    /// Reserved for the tagging slice, as [`Field::tags`].
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// A case: the field values written against a template, keyed by field id.
#[derive(Debug, Clone)]
pub struct Case {
    pub case_id: String,
    pub case_arn: String,
    pub domain_id: String,
    pub template_id: String,
    /// The union variant each value was written with is preserved exactly, so a
    /// read hands back what was written rather than a re-encoding of it. Keyed
    /// by field id, so it is bounded by [`MAX_FIELDS_PER_DOMAIN`].
    pub field_values: HashMap<String, FieldValueUnion>,
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub performed_by: Option<UserUnion>,
    /// This case's audit history: one entry per accepted edit of the case, in
    /// the order the edits were made (so the back is the most recent one).
    /// Bounded by [`MAX_AUDIT_EVENTS_PER_CASE`] — a deque because the ceiling
    /// drops the oldest event rather than refusing a further edit. It is part
    /// of the case, so deleting the case takes its audit history with it, which
    /// is what `DeleteCase` documents ("After a successful deletion, you cannot
    /// ... Access audit history").
    pub audit_events: VecDeque<CaseAuditEvent>,
    /// How many audit events this case has recorded in all, including any the
    /// ceiling has already dropped. It is what orders the history
    /// ([`audit_order_key`]): two edits made in the same instant still have
    /// distinct ordinals, so a page boundary in the history is stable.
    pub audit_events_recorded: u64,
    /// The related items associated with this case, keyed by `relatedItemId`.
    /// Bounded by [`MAX_RELATED_ITEMS_PER_CASE`]. They belong to the case, so
    /// deleting the case takes its associations with it.
    pub related_items: HashMap<String, RelatedItem>,
}

/// One related item associated with a case: a resource of its own, with its own
/// identifier and ARN, which "may or may not have an external identifier linking
/// it to an external resource" (`CreateRelatedItem`'s documentation).
#[derive(Debug, Clone)]
pub struct RelatedItem {
    pub related_item_id: String,
    pub related_item_arn: String,
    /// The case this item is associated with.
    pub case_id: String,
    /// The item's `RelatedItemType`, one of `RELATED_ITEM_TYPE_VALUES`. It is
    /// also the member [`RelatedItem::content`] carries: a create whose `type`
    /// does not name its content's member is refused, so the two agree.
    pub item_type: String,
    /// The content exactly as the association was written with it, in the union
    /// variant it arrived in, so a read hands back the record rather than a
    /// re-encoding of it.
    pub content: RelatedItemInputContent,
    /// When the item was associated with the case, which is what
    /// `SearchRelatedItems` reports as its `associationTime`.
    pub association_time: SystemTime,
    /// The entity the request named as having performed the association, when it
    /// named one: the item's creator.
    pub performed_by: Option<UserUnion>,
    /// Reserved for the tagging slice, as [`Field::tags`].
    pub tags: HashMap<String, String>,
}

/// One recorded edit of a case: what a page of its audit history reports.
#[derive(Debug, Clone)]
pub struct CaseAuditEvent {
    pub event_id: String,
    /// One of `AUDIT_EVENT_TYPE_VALUES`.
    pub event_type: String,
    /// The `RelatedItemType` of the related item an event of the
    /// `RelatedItem.*` types is about; absent for an edit of the case itself.
    pub related_item_type: Option<String>,
    pub performed_time: SystemTime,
    /// The ordinal this event was recorded at, from
    /// [`Case::audit_events_recorded`].
    pub ordinal: u64,
    /// The field values this edit wrote. Bounded by [`MAX_CASE_FIELD_IDS`],
    /// the cap on the field list of the request it was built from.
    pub fields: Vec<CaseAuditEventField>,
    /// The entity the request that made this edit named as having performed it,
    /// when it named one.
    pub performed_by: Option<UserUnion>,
    /// The IAM principal whose credentials made the edit
    /// ([`caller_principal_arn`]).
    pub iam_principal_arn: String,
}

/// One field an edit of a case wrote: the value the case held for it before the
/// edit (absent when it held none) and the value the edit stored.
#[derive(Debug, Clone)]
pub struct CaseAuditEventField {
    pub field_id: String,
    pub old_value: Option<FieldValueUnion>,
    pub new_value: FieldValueUnion,
}

/// A remembered `CreateCase` idempotency token: the domain it was used in and
/// the token itself. The domain is part of the key because the same token used
/// against a different domain is a different request.
type IdempotencyKey = (String, String);

/// Per-service state for com-amazonaws-connectcases-amazonconnectcases. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcasesAmazonconnectcasesState {
    /// Every domain, keyed by `domainId`. Bounded by [`MAX_DOMAINS`].
    pub domains: HashMap<String, Domain>,
    /// `(domainId, CreateCase clientToken)` -> the `caseId` that token already
    /// minted, so a replay resolves to the same case instead of making another.
    /// Reached through [`Self::idempotent_case`] / [`Self::remember_idempotent_case`]
    /// so the eviction order below cannot drift from it.
    case_idempotency: HashMap<IdempotencyKey, String>,
    /// Insertion order of `case_idempotency`, so the OLDEST remembered token is
    /// the one dropped at [`MAX_IDEMPOTENCY_TOKENS`].
    idempotency_order: VecDeque<IdempotencyKey>,
}

impl ComAmazonawsConnectcasesAmazonconnectcasesState {
    /// The domain `domain_id` names, or the modelled 404 for it.
    pub fn domain(&self, domain_id: &str) -> Result<&Domain, ResourceNotFoundException> {
        self.domains
            .get(domain_id)
            .ok_or_else(|| not_found(ResourceKind::Domain, domain_id))
    }

    /// The domain `domain_id` names for writing, or the modelled 404 for it,
    /// worded with the service's generic sentence.
    pub fn domain_mut(&mut self, domain_id: &str) -> Result<&mut Domain, ResourceNotFoundException> {
        self.domain_mut_worded(domain_id, NotFoundWording::Generic)
    }

    /// The domain `domain_id` names for writing, or the modelled 404 for it
    /// worded as `wording` words it: what an operation that sends a sentence of
    /// its own for a domain it cannot resolve resolves the domain through.
    pub fn domain_mut_worded(
        &mut self,
        domain_id: &str,
        wording: NotFoundWording,
    ) -> Result<&mut Domain, ResourceNotFoundException> {
        self.domains
            .get_mut(domain_id)
            .ok_or_else(|| not_found_worded(ResourceKind::Domain, domain_id, wording))
    }

    /// Refuse a new domain once the store holds [`MAX_DOMAINS`] of them.
    pub fn domain_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(self.domains.len(), MAX_DOMAINS, "Cases domains in this store")
    }

    /// Every domain this store holds, in creation order and each paired with the
    /// key [`order_key`] builds for it: what a listing of domains reports, in the
    /// order [`page`] needs it in. The store keeps its domains in a hash map, so
    /// the order is imposed here, exactly as [`Domain::live_templates`] does for a
    /// domain's templates.
    pub fn domains_in_creation_order(&self) -> Vec<(String, &Domain)> {
        let mut domains: Vec<(String, &Domain)> = self
            .domains
            .values()
            .map(|domain| (order_key(domain.created_time, &domain.domain_id), domain))
            .collect();
        domains.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        domains
    }

    /// The case the `client_token` already minted in `domain_id`, if the store
    /// still remembers that token AND the case it names still exists.
    pub fn idempotent_case(&self, domain_id: &str, client_token: &str) -> Option<&Case> {
        let case_id = self
            .case_idempotency
            .get(&(domain_id.to_string(), client_token.to_string()))?;
        self.domains.get(domain_id)?.cases.get(case_id)
    }

    /// Remember that `client_token` minted `case_id` in `domain_id`, evicting
    /// the oldest remembered token once [`MAX_IDEMPOTENCY_TOKENS`] is reached.
    pub fn remember_idempotent_case(
        &mut self,
        domain_id: &str,
        client_token: &str,
        case_id: &str,
    ) {
        let key = (domain_id.to_string(), client_token.to_string());
        if self
            .case_idempotency
            .insert(key.clone(), case_id.to_string())
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.idempotency_order.pop_front() {
                self.case_idempotency.remove(&evicted);
            }
        }
    }

    /// Delete the domain `domain_id` names, with everything scoped to it (its
    /// fields, templates, layouts, cases and their related items and audit
    /// history), or answer the modelled 404 for a domain this store does not
    /// hold.
    ///
    /// The domain is the container every other resource of this service hangs
    /// off, so dropping the record drops the whole subtree with it: nothing can
    /// resolve a resource of a domain that no longer exists. The remembered
    /// `CreateCase` tokens of that domain go too — a token that outlived its
    /// domain would answer a replay with the id of a case nothing holds.
    pub fn remove_domain(&mut self, domain_id: &str) -> Result<Domain, ResourceNotFoundException> {
        let removed = self
            .domains
            .remove(domain_id)
            .ok_or_else(|| not_found(ResourceKind::Domain, domain_id))?;
        self.case_idempotency
            .retain(|(domain, _), _| domain != domain_id);
        self.idempotency_order
            .retain(|(domain, _)| domain != domain_id);
        Ok(removed)
    }

    /// Forget every token that resolved to `case_id` in `domain_id`, so a
    /// replay cannot answer with the id of a case that no longer exists.
    pub fn forget_idempotent_case(&mut self, domain_id: &str, case_id: &str) {
        let dropped: Vec<IdempotencyKey> = self
            .case_idempotency
            .iter()
            .filter(|((domain, _), stored)| domain == domain_id && stored.as_str() == case_id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in dropped {
            self.case_idempotency.remove(&key);
            self.idempotency_order.retain(|existing| existing != &key);
        }
    }
}

impl Domain {
    /// The case `case_id` names, or the modelled 404 for it, worded as a call
    /// that ADDRESSES a case words it ([`NotFoundWording::AddressedCase`]).
    pub fn case(&self, case_id: &str) -> Result<&Case, ResourceNotFoundException> {
        self.cases.get(case_id).ok_or_else(|| {
            not_found_worded(ResourceKind::Case, case_id, NotFoundWording::AddressedCase)
        })
    }

    /// The LIVE template `template_id` names, or the modelled 404 for it. A
    /// template that has been deleted is not one a caller can take out a new
    /// reference to, so the paths that write through a template — an update of
    /// it, a case created from it — resolve it here.
    pub fn template(&self, template_id: &str) -> Result<&Template, ResourceNotFoundException> {
        self.templates
            .get(template_id)
            .filter(|template| !template.deleted)
            .ok_or_else(|| not_found(ResourceKind::Template, template_id))
    }

    /// The RECORD this domain holds for `template_id`, live or deleted, or
    /// `None` when it holds none at all.
    ///
    /// A template's delete is a SOFT delete, so a deleted template is a
    /// tombstone rather than an absence. The read the service documents as
    /// surviving the delete resolves a template through this, and
    /// [`Template::deleted`] on what comes back says which of the two it is.
    pub fn template_record(&self, template_id: &str) -> Option<&Template> {
        self.templates.get(template_id)
    }

    /// The live field `field_id` names, or the modelled 404 for it. A field that
    /// has been deleted is gone as far as a reader is concerned, exactly as
    /// [`Self::template`] treats a deleted template.
    pub fn field(&self, field_id: &str) -> Result<&Field, ResourceNotFoundException> {
        self.fields
            .get(field_id)
            .filter(|field| !field.deleted)
            .ok_or_else(|| not_found(ResourceKind::Field, field_id))
    }

    /// The live field `field_id` names, for writing, or the modelled 404 for it.
    pub fn field_mut(&mut self, field_id: &str) -> Result<&mut Field, ResourceNotFoundException> {
        self.fields
            .get_mut(field_id)
            .filter(|field| !field.deleted)
            .ok_or_else(|| not_found(ResourceKind::Field, field_id))
    }

    /// The RECORD this domain holds for `field_id`, live or deleted, or `None`
    /// when it holds none at all.
    ///
    /// A field's delete is a SOFT delete, so a deleted field is a tombstone
    /// rather than an absence: its id, its declared type and the case values
    /// written against it are all still defined. The reads the service
    /// documents as surviving a delete resolve a field through this, and
    /// [`Field::deleted`] on what comes back says which of the two it is.
    pub fn field_record(&self, field_id: &str) -> Option<&Field> {
        self.fields.get(field_id)
    }

    /// Whether this domain holds `field_id` as a DELETED field: a record that
    /// exists and is flagged.
    ///
    /// What the reports of a field list filter on: they drop an id whose field
    /// was deleted, and leave every other id they stored exactly as stored.
    pub fn is_deleted_field(&self, field_id: &str) -> bool {
        self.fields.get(field_id).is_some_and(|field| field.deleted)
    }

    /// Every template this domain still holds, in creation order and each
    /// paired with the key [`order_key`] builds for it, with the deleted ones
    /// left out: what a listing of templates reports before the request's own
    /// filters narrow it. A deleted template is left out of the LISTING only —
    /// [`Self::template_record`] still reports the record, because
    /// `DeleteTemplate` says a deleted template is still retrievable through
    /// `GetTemplate` while "Deleted templates are not included in the
    /// ListTemplates response".
    pub fn live_templates(&self) -> Vec<(String, &Template)> {
        let mut templates: Vec<(String, &Template)> = self
            .templates
            .values()
            .filter(|template| !template.deleted)
            .map(|template| {
                (
                    order_key(template.created_time, &template.template_id),
                    template,
                )
            })
            .collect();
        templates.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        templates
    }

    /// Every field this domain still holds, in creation order and each paired
    /// with the key [`order_key`] builds for it, with the deleted ones left out:
    /// what a listing of fields reports. A deleted field is left out because the
    /// model says so for that listing in particular — "Deleted fields are not
    /// included in the `ListFields` response"
    /// (`smithy_query(query="doc", shape="DeleteField")`) — and the same
    /// tombstone is what [`Self::unknown_field_ids`] already treats as absent.
    pub fn live_fields(&self) -> Vec<(String, &Field)> {
        let mut fields: Vec<(String, &Field)> = self
            .fields
            .values()
            .filter(|field| !field.deleted)
            .map(|field| (order_key(field.created_time, &field.field_id), field))
            .collect();
        fields.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        fields
    }

    /// Refuse a new field once the domain holds [`MAX_FIELDS_PER_DOMAIN`] of
    /// them. Only the fields a CALLER made are counted: the ceiling bounds
    /// request-driven growth, and the reserved `System` fields
    /// ([`RESERVED_SYSTEM_FIELDS`]) the service provisions the domain with are
    /// not a caller's doing, so they do not eat a caller's budget.
    pub fn field_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        let created_by_a_caller = self
            .fields
            .values()
            .filter(|field| field.namespace != FIELD_NAMESPACE_SYSTEM)
            .count();
        capacity(
            created_by_a_caller,
            MAX_FIELDS_PER_DOMAIN,
            "fields in this domain",
        )
    }

    /// Refuse a new template once the domain holds [`MAX_TEMPLATES_PER_DOMAIN`].
    pub fn template_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.templates.len(),
            MAX_TEMPLATES_PER_DOMAIN,
            "templates in this domain",
        )
    }

    /// The layout `layout_id` names, or the modelled 404 for it. A layout that
    /// has been deleted IS still reported, unlike a template: per
    /// `DeleteLayout`'s documentation, after a delete "You can still retrieve
    /// the layout by calling GetLayout".
    pub fn layout(&self, layout_id: &str) -> Result<&Layout, ResourceNotFoundException> {
        self.layouts
            .get(layout_id)
            .ok_or_else(|| not_found(ResourceKind::Layout, layout_id))
    }

    /// Every layout this domain still holds, in creation order and each paired
    /// with the key [`order_key`] builds for it, with the deleted ones left out:
    /// what a listing of layouts reports. A deleted layout is left out of the
    /// LISTING only — [`Self::layout`] still reports the record, because
    /// `DeleteLayout` says a deleted layout is still retrievable through
    /// `GetLayout` and names no other read it survives in.
    pub fn live_layouts(&self) -> Vec<(String, &Layout)> {
        let mut layouts: Vec<(String, &Layout)> = self
            .layouts
            .values()
            .filter(|layout| !layout.deleted)
            .map(|layout| (order_key(layout.created_time, &layout.layout_id), layout))
            .collect();
        layouts.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        layouts
    }

    /// Refuse a new layout once the domain holds [`MAX_LAYOUTS_PER_DOMAIN`].
    pub fn layout_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.layouts.len(),
            MAX_LAYOUTS_PER_DOMAIN,
            "layouts in this domain",
        )
    }

    /// Refuse a new case once the domain holds [`MAX_CASES_PER_DOMAIN`].
    pub fn case_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.cases.len(),
            MAX_CASES_PER_DOMAIN,
            "cases in this domain",
        )
    }

    /// The ids among `ids` that this domain holds no live field for, in the
    /// order they were given and without repeats. Empty means every reference
    /// resolves. Linear in the number of ids: one hash lookup each.
    ///
    /// What a request that takes out a NEW reference to a field resolves
    /// against: a deleted field is not part of the domain's live schema, so a
    /// reference to one does not resolve here.
    pub fn unknown_field_ids<'a, I: IntoIterator<Item = &'a str>>(&self, ids: I) -> Vec<String> {
        self.field_ids_matching(ids, |field| {
            field.is_none_or(|field: &Field| field.deleted)
        })
    }

    /// The ids among `ids` that this domain holds NO record for at all, live or
    /// deleted, in the order they were given and without repeats.
    ///
    /// What an operation that reads or rewrites a value already written against
    /// a field resolves against: a deleted field's record is still defined, so
    /// its id resolves here while [`Self::unknown_field_ids`] rejects it.
    pub fn unrecorded_field_ids<'a, I: IntoIterator<Item = &'a str>>(&self, ids: I) -> Vec<String> {
        self.field_ids_matching(ids, |field| field.is_none())
    }

    /// The ids among `ids` whose record this domain holds as a DELETED field,
    /// in the order they were given and without repeats: the ids a refusal has
    /// to name as deleted rather than as unresolvable.
    pub fn deleted_field_ids<'a, I: IntoIterator<Item = &'a str>>(&self, ids: I) -> Vec<String> {
        self.field_ids_matching(ids, |field| {
            field.is_some_and(|field: &Field| field.deleted)
        })
    }

    /// The ids among `ids` whose record in this domain satisfies `matches`,
    /// each reported once in first-seen order. One hash lookup per id, so it is
    /// linear in the number of ids; the three classifications above are the one
    /// loop, so they cannot drift apart.
    fn field_ids_matching<'a, I, F>(&self, ids: I, matches: F) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
        F: Fn(Option<&Field>) -> bool,
    {
        let mut reported: HashSet<&str> = HashSet::new();
        let mut selected: Vec<String> = Vec::new();
        for id in ids {
            if matches(self.fields.get(id)) && reported.insert(id) {
                selected.push(id.to_string());
            }
        }
        selected
    }

    /// The ids among `values` whose union variant is not a value of the
    /// declared `FieldType` of the field it is written against, each reported
    /// once in first-seen order with the type and the variant named. An id the
    /// domain holds no record for at all is left to
    /// [`Self::unrecorded_field_ids`]. A DELETED field still declares its type,
    /// and a value may still be written against it, so a value written against
    /// a tombstone is type-checked like any other. Linear in the number of
    /// values.
    pub fn mismatched_field_values<'a, I>(&self, values: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, &'a FieldValueUnion)>,
    {
        let mut reported: HashSet<&str> = HashSet::new();
        let mut mismatched: Vec<String> = Vec::new();
        for (id, value) in values {
            let Some(field) = self.field_record(id) else {
                continue;
            };
            if !value_fits_field_type(&field.field_type, value) && reported.insert(id) {
                mismatched.push(format!(
                    "{id} is a {} field and cannot hold {}",
                    field.field_type,
                    variant_name(value)
                ));
            }
        }
        mismatched
    }
}

/// A case's audit history: the record of the edits made to it.
impl Case {
    /// Refuse a further association once the case holds
    /// [`MAX_RELATED_ITEMS_PER_CASE`] related items.
    pub fn related_item_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.related_items.len(),
            MAX_RELATED_ITEMS_PER_CASE,
            "related items on this case",
        )
    }

    /// Append one recorded edit to this case's audit history, dropping the
    /// OLDEST event once the history holds more than
    /// [`MAX_AUDIT_EVENTS_PER_CASE`], so a caller cannot grow one case's
    /// history without end.
    ///
    /// The ordinal keeps rising across a drop, so the order of what remains is
    /// unaffected by eviction and a cursor into the history never points before
    /// the first event the case still holds.
    pub fn record_audit_event(
        &mut self,
        event_type: &str,
        related_item_type: Option<String>,
        performed_time: SystemTime,
        iam_principal_arn: String,
        performed_by: Option<UserUnion>,
        fields: Vec<CaseAuditEventField>,
    ) {
        let ordinal = self.audit_events_recorded;
        self.audit_events_recorded = self.audit_events_recorded.saturating_add(1);
        self.audit_events.push_back(CaseAuditEvent {
            event_id: new_id(),
            event_type: event_type.to_string(),
            // Only an edit of a RELATED item carries one; an edit of the case
            // itself is about no related item and reports none.
            related_item_type,
            performed_time,
            ordinal,
            fields,
            performed_by,
            iam_principal_arn,
        });
        while self.audit_events.len() > MAX_AUDIT_EVENTS_PER_CASE {
            self.audit_events.pop_front();
        }
    }

    /// This case's audit history most recent edit first, each event paired with
    /// the key [`audit_order_key`] builds for it: what a page of the history
    /// reports, in the order [`page`] needs it in.
    pub fn audit_history_newest_first(&self) -> Vec<(String, &CaseAuditEvent)> {
        self.audit_events
            .iter()
            .rev()
            .map(|event| (audit_order_key(event.ordinal, &event.event_id), event))
            .collect()
    }
}

/// The key a case's audit history is ordered by: the ordinal of the recorded
/// edit, COMPLEMENTED so that lexicographic order over this key is most-recent
/// first, with the event's own id after it so two events can never share a key.
///
/// Reporting an event history newest first is what makes the first page the
/// useful one for a caller that only wants the latest edits, and it is the order
/// the emulator reports every page of a case's history in — the model states no
/// order for `auditEvents`, so this is the emulator's choice, stated here once
/// and applied to every page.
pub fn audit_order_key(ordinal: u64, event_id: &str) -> String {
    format!("{:020}#{event_id}", u64::MAX - ordinal)
}

/// The audit-history fields one edit of a case records: one entry per field
/// value the request wrote, in the order the request named them, each carrying
/// the value the case held for that field before the edit and the value the edit
/// stored.
///
/// `before` is the case's field values as they stood before the edit; `None` is
/// a case that did not exist yet, where no field can have held a previous value.
/// Linear in the number of values written, and bounded with it by
/// [`MAX_CASE_FIELD_IDS`].
pub fn audit_event_fields<'a, I>(
    before: Option<&HashMap<String, FieldValueUnion>>,
    written: I,
) -> Vec<CaseAuditEventField>
where
    I: IntoIterator<Item = (&'a str, &'a FieldValueUnion)>,
{
    written
        .into_iter()
        .map(|(field_id, new_value)| CaseAuditEventField {
            field_id: field_id.to_string(),
            old_value: before.and_then(|held| held.get(field_id)).cloned(),
            new_value: new_value.clone(),
        })
        .collect()
}

/// The IAM principal an audit record attributes an edit to, as
/// `AuditEventPerformedBy$iamPrincipalArn` carries it: the IAM user the caller's
/// own SigV4 access key id stands for.
///
/// Derived from the credential scope of the request that made the edit, so the
/// same caller is the same principal on every call and nothing in it is minted
/// per request. It is stored with the event, so a later read reports the
/// principal that made THAT edit rather than the one reading the history.
pub fn caller_principal_arn(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    ::engine::emulator_arn("iam", "", &format!("user/{access_key_id}"))
}

/// Field options: the set a single-select field offers.
impl Field {
    /// Refuse `adding` new options once the field would hold more than
    /// [`MAX_FIELD_OPTIONS_PER_FIELD`]. `adding` counts only values the field
    /// does not hold yet, since putting a value it already holds updates that
    /// option rather than growing the set.
    pub fn option_capacity(&self, adding: usize) -> Result<(), ServiceQuotaExceededException> {
        if self.options.len() + adding <= MAX_FIELD_OPTIONS_PER_FIELD {
            Ok(())
        } else {
            Err(ServiceQuotaExceededException {
                message: format!(
                    "the emulator holds at most {MAX_FIELD_OPTIONS_PER_FIELD} field options per \
                     field; this field already holds {} and the request adds {adding} more",
                    self.options.len()
                ),
            })
        }
    }
}

/// Whether `value` is a well-formed `FieldOptionValue`.
///
/// The model's own `pattern` on `FieldOptionValue` only forbids a trailing
/// space, but its documentation states the rule the service enforces:
/// "`FieldOptionValue` has max length 100 and must be alphanumeric with hyphens
/// and underscores"
/// (`smithy_query(query="shape", shape="FieldOption")`, member `value`). A value
/// outside that character set therefore passes the boundary validator and has to
/// be refused here — per option, through `BatchPutFieldOptions`'s own `errors`
/// list, which exists for exactly that. The length bound is the model's and is
/// restated so this one function answers "is this a value the service accepts".
pub fn field_option_value_is_well_formed(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_FIELD_OPTION_TEXT_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Longest `FieldOptionName` / `FieldOptionValue` the model accepts: both are
/// `length 1..100` (`smithy_query(query="shape", shape="FieldOptionValue")`).
pub const MAX_FIELD_OPTION_TEXT_LEN: usize = 100;

/// `Ok` while `held` is below `limit`, else the modelled quota error naming
/// what filled up.
fn capacity(
    held: usize,
    limit: usize,
    what: &str,
) -> Result<(), ServiceQuotaExceededException> {
    if held < limit {
        Ok(())
    } else {
        Err(ServiceQuotaExceededException {
            message: format!(
                "the emulator holds at most {limit} {what}; delete one before creating another"
            ),
        })
    }
}

/// The `FieldValueUnion` variant name, for a refusal that has to say which
/// variant it saw.
pub fn variant_name(value: &FieldValueUnion) -> &'static str {
    match value {
        FieldValueUnion::stringValue(_) => "stringValue",
        FieldValueUnion::doubleValue(_) => "doubleValue",
        FieldValueUnion::booleanValue(_) => "booleanValue",
        FieldValueUnion::emptyValue(_) => "emptyValue",
        FieldValueUnion::userArnValue(_) => "userArnValue",
    }
}

/// Whether `value`'s union variant is a value OF `field_type`.
///
/// A case field's value has the field's own declared type, so the variant and
/// the `FieldType` have to agree: a `Boolean` field holds `booleanValue`, a
/// `Number` field `doubleValue`, a `User` field `userArnValue`, and the
/// string-valued types (`DateTime`, `SingleSelect`, `Text`, `Url`) hold
/// `stringValue`. `emptyValue` is the absent value and clears a field of any
/// type. `field_type` is one of `FIELD_TYPE_VALUES`; an unrecognised one
/// matches nothing, because a value cannot be checked against a type this code
/// does not know.
pub fn value_fits_field_type(field_type: &str, value: &FieldValueUnion) -> bool {
    match value {
        FieldValueUnion::emptyValue(_) => true,
        FieldValueUnion::booleanValue(_) => field_type == "Boolean",
        FieldValueUnion::doubleValue(_) => field_type == "Number",
        FieldValueUnion::userArnValue(_) => field_type == "User",
        FieldValueUnion::stringValue(_) => {
            matches!(field_type, "DateTime" | "SingleSelect" | "Text" | "Url")
        }
    }
}

/// The ids that appear more than once in `ids`, each reported once in first-seen
/// order. Empty means every id is distinct. Linear in the number of ids.
///
/// Used by the WRITE paths (`CreateCase`, `UpdateCase`, `CreateTemplate`), where
/// one id carrying two values — or one template naming the same required field
/// twice — has no defined meaning, so last-one-wins would be an arbitrary
/// choice. The reads do not use it: the model declares no distinctness
/// constraint and they answer one entry per occurrence.
pub fn duplicate_ids<'a, I: IntoIterator<Item = &'a str>>(ids: I) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut reported: HashSet<&str> = HashSet::new();
    let mut duplicates: Vec<String> = Vec::new();
    for id in ids {
        if !seen.insert(id) && reported.insert(id) {
            duplicates.push(id.to_string());
        }
    }
    duplicates
}

/// The ids among `required` that `provided` does not name, each reported once in
/// first-seen order. Empty means every required id has an entry.
///
/// A template's `requiredFields` is, per the model, the "List of fields that
/// must have a value provided to create a case", so what this checks is exactly
/// that: an id is PROVIDED when the request's `fields` carries an entry for it,
/// whatever union variant that entry holds — `emptyValue` is a value of the
/// union ("An empty value"), so it counts as provided and the rule is not
/// widened past what the model states. Linear in both lists.
pub fn ids_not_provided<'a, I: IntoIterator<Item = &'a str>>(
    required: &[String],
    provided: I,
) -> Vec<String> {
    let provided: HashSet<&str> = provided.into_iter().collect();
    let mut reported: HashSet<&str> = HashSet::new();
    let mut missing: Vec<String> = Vec::new();
    for id in required {
        if !provided.contains(id.as_str()) && reported.insert(id.as_str()) {
            missing.push(id.clone());
        }
    }
    missing
}

/// `Some(message)` when a request's `member` list is longer than `cap`: the
/// refusal text naming the member, the length seen and the ceiling.
///
/// Checked in the handler even though the model declares the same cap, so the
/// per-request work (and the response built from it) is bounded by this code
/// rather than only by the boundary validator.
pub fn list_too_long(member: &str, len: usize, cap: usize) -> Option<String> {
    (len > cap).then(|| {
        format!("{member} carries {len} entries; at most {cap} are accepted in one request")
    })
}

/// The modelled 400 for a request that references field ids the domain's schema
/// does not define, as [`Domain::unknown_field_ids`] reports them.
///
/// One wording for one rule: whichever member carried the references, the
/// service names the ids it could not resolve after its own fixed text, so every
/// operation that resolves a field id against a domain refuses in the same
/// words. The ids come from the REQUEST, verbatim, and are rendered by
/// [`name_ids`] so one request cannot make the refusal grow without bound.
pub fn unknown_fields(ids: &[String]) -> ValidationException {
    ValidationException {
        message: format!("Unknown field Ids: {}", name_ids(ids)),
    }
}

/// The modelled 400 for a request a DELETED field id cannot be used in, as
/// [`Domain::deleted_field_ids`] reports them.
///
/// The service's answer for such a request "denot[es] which field identifiers
/// in the request have been deleted"
/// (`smithy_query(query="doc",shape="DeleteField")`), so the ids come from the
/// REQUEST, verbatim, after the service's own fixed text, and are rendered by
/// [`name_ids`] so one request cannot make the refusal grow without bound.
pub fn deleted_fields(ids: &[String]) -> ValidationException {
    ValidationException {
        message: format!("Deleted field Ids: {}", name_ids(ids)),
    }
}

/// The modelled 400 for a create refused because the template it names requires
/// a value for `field_id` and the request carried no entry for it, as
/// [`ids_not_provided`] reports them.
///
/// The sentence names the one required field the request did not provide — the
/// first of them, in the order the template declares its `requiredFields` — and
/// nothing else: the template is addressed by the request, so its id is not
/// interpolated into the refusal.
pub fn required_field_not_provided(field_id: &str) -> ValidationException {
    ValidationException {
        message: format!("Field value is not provided for a required field: {field_id}"),
    }
}

/// `ids` rendered for a refusal message, naming at most
/// [`MAX_IDS_NAMED_IN_A_REFUSAL`] of them and counting the rest.
pub fn name_ids(ids: &[String]) -> String {
    if ids.len() <= MAX_IDS_NAMED_IN_A_REFUSAL {
        ids.join(", ")
    } else {
        format!(
            "{}, and {} more",
            ids[..MAX_IDS_NAMED_IN_A_REFUSAL].join(", "),
            ids.len() - MAX_IDS_NAMED_IN_A_REFUSAL
        )
    }
}

/// Most entries one page of a listing holds. The model's own cap, stated twice
/// for every paginated listing in this service: `MaxResults` is `@range(1..100)`
/// and the summary list a response carries is `@length(max: 100)`
/// (`smithy_query(query="operation", operation="ListTemplates")`). A request
/// that names no `maxResults` therefore gets at most this many entries and a
/// `nextToken` for the rest, never the whole store in one answer.
pub const MAX_PAGE_SIZE: i32 = 100;

/// One page of a listing: the entries on it, and the token that resumes after
/// them (`None` when nothing remains).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Why a listing refused its paging arguments.
#[derive(Debug)]
pub enum PageError {
    /// `nextToken` was not a token this service issued.
    InvalidToken(String),
    /// `maxResults` was not a page size in the modelled range.
    InvalidMaxResults(i32),
}

/// Take one page out of an ORDERED listing.
///
/// `all` is every entry that matches the request, each paired with the key the
/// listing is ordered by (see [`order_key`]) and already sorted by it. The page
/// holds at most `max_results` entries starting at `next_token`, and carries a
/// token only when further entries remain — so a caller that follows tokens to
/// exhaustion sees every entry exactly once, and a page that is not full is the
/// last one.
///
/// `scope` identifies the LISTING: the operation, the container it reports and
/// the filters that narrowed it (see [`page_scope`]). It is stamped into the
/// token, and a token presented to a different listing is refused rather than
/// resumed from — resuming one listing from another listing's cursor would
/// answer a page of unrelated entries, starting at an arbitrary offset.
///
/// The token names the first entry NOT on the page rather than a server-side
/// cursor, so paging holds no per-caller state and nothing a listing hands out
/// has to be remembered.
pub fn page<T>(
    scope: &str,
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, PageError> {
    // `maxResults` first: a request that is wrong in both arguments is told
    // about the page size, which is the one it can fix without another call.
    let limit = match max_results {
        None => MAX_PAGE_SIZE as usize,
        // The modelled range is enforced in both directions: a page size the
        // model does not admit is refused, never quietly clamped to one it does.
        Some(requested) if (1..=MAX_PAGE_SIZE).contains(&requested) => requested as usize,
        Some(requested) => return Err(PageError::InvalidMaxResults(requested)),
    };
    // An empty `nextToken` is a value the model's `@length(0..9000)` admits and
    // means "from the start", the same as not sending one.
    let start = match next_token.filter(|token| !token.is_empty()) {
        None => None,
        Some(token) => Some(
            decode_page_token(scope, token)
                .ok_or_else(|| PageError::InvalidToken(token.to_string()))?,
        ),
    };

    let mut rest = all
        .into_iter()
        .skip_while(|(key, _)| start.as_deref().is_some_and(|from| key.as_str() < from));
    let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
    let next_token = rest.next().map(|(key, _)| encode_page_token(scope, &key));
    Ok(Page { items, next_token })
}

/// The scope of one listing, for [`page`]: the operation, the container whose
/// resources it reports, and the filter values that narrowed it, in a form two
/// calls of the SAME listing agree on — the filters are sorted, so naming the
/// same values in another order is the same listing. The components are joined
/// for readability only: a token stamps a DIGEST of the whole scope
/// ([`encode_page_token`]), so two different scopes cannot collide by how their
/// components happen to be punctuated.
pub fn page_scope(operation: &str, container: &str, filters: &[String]) -> String {
    let mut filters: Vec<&str> = filters.iter().map(String::as_str).collect();
    filters.sort_unstable();
    format!("{operation}/{container}/{}", filters.join(","))
}

/// The modelled 400 for paging arguments a listing cannot honour, naming the
/// member a client has to fix.
pub fn page_validation_exception(error: &PageError) -> ValidationException {
    match error {
        // The token is named, but only its leading characters: the model admits a
        // `nextToken` of up to 9000 characters, and a refusal does not echo a
        // whole request member back — the same discipline [`name_ids`] applies.
        PageError::InvalidToken(token) => ValidationException {
            message: format!(
                "nextToken {} is not a pagination token this service issued for this listing",
                abbreviate(token)
            ),
        },
        PageError::InvalidMaxResults(requested) => {
            max_results_out_of_range(*requested, MAX_PAGE_SIZE)
        }
    }
}

/// The modelled 400 for a page size outside `1..=cap`, which is the one message
/// template this service refuses a page size with — the cap differs per
/// operation (compare [`MAX_PAGE_SIZE`] with
/// [`MAX_AUDIT_EVENTS_PER_PAGE`]), the wording does not.
pub fn max_results_out_of_range(requested: i32, cap: i32) -> ValidationException {
    ValidationException {
        message: format!("maxResults must be between 1 and {cap}, but was {requested}"),
    }
}

/// Most characters of a caller-supplied token a refusal repeats, so a refusal
/// over a long member cannot be amplified into a long response.
pub const MAX_TOKEN_CHARS_IN_A_REFUSAL: usize = 24;

/// `value` as a refusal names it: itself when short, else its first
/// [`MAX_TOKEN_CHARS_IN_A_REFUSAL`] characters and the length of the rest.
fn abbreviate(value: &str) -> String {
    if value.chars().count() <= MAX_TOKEN_CHARS_IN_A_REFUSAL {
        value.to_string()
    } else {
        let head: String = value.chars().take(MAX_TOKEN_CHARS_IN_A_REFUSAL).collect();
        format!(
            "{head}... ({} characters in all)",
            value.chars().count()
        )
    }
}

/// The key a listing of resources created in one container is ordered by:
/// creation instant first, the resource's own id to break a tie between two
/// created in the same instant. Lexicographic order over this key is creation
/// order, which is what makes a page boundary stable — a listing has to iterate
/// in the same order for the call that hands out a `nextToken` and the call that
/// resumes from it, and the store keeps its resources in a hash map.
pub fn order_key(created_time: SystemTime, id: &str) -> String {
    let nanos = created_time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:039}#{id}")
}

/// Encode a listing key as an opaque continuation token.
///
/// The key is prefixed with a version marker and a FIXED-WIDTH digest of the
/// listing's own `scope` before hex-encoding, so the token is opaque to a client
/// (it is not the key it was built from) AND a value this service did not issue
/// FOR THIS LISTING is refused rather than resumed from: hex alone would accept
/// any even-length hex string as a cursor, and a service-wide marker alone would
/// let one listing's token resume another. The digest is what makes the scope /
/// key boundary unambiguous whatever the scope's components contain, so no
/// separator convention has to hold for a token to decode to the right listing.
/// It stays a pure function of the scope and the key, so the token is stable and
/// carries no server-side cursor state.
fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_PREFIX}{}{key}", scope_digest(scope)))
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did
/// not issue for the listing `scope` names.
fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{PAGE_TOKEN_PREFIX}{}", scope_digest(scope)))
        .map(str::to_string)
}

/// `scope` as a fixed-width hex digest, so a token's scope stamp occupies the
/// same number of characters for every listing and cannot run into the order key
/// after it. SHA-256 from `sha2`, as the ids are minted ([`new_id`]); this crate
/// implements no hash of its own.
fn scope_digest(scope: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// Marks a continuation token as this service's, and at which version it was
/// issued.
const PAGE_TOKEN_PREFIX: &str = "connectcases:v1:";

/// A fresh opaque resource id, in the textual form the real service's ids take
/// (a hyphenated, version-4 UUID): SHA-256 over this process's OS-seeded
/// entropy, a monotonic mint counter and the mint instant.
///
/// Nothing in it comes from the request — two ids minted for byte-identical
/// input differ, because the counter does — and nothing about it is guessable
/// from another id, because the per-process entropy behind it never leaves this
/// process. The counter is what makes the ids UNIQUE; the entropy is what makes
/// them UNPREDICTABLE.
///
/// The digest and the hex codec come from `sha2`/`hex`, as the sibling service
/// crates mint their opaque ids, and the entropy from `std`'s OS-seeded
/// [`RandomState`]; this crate implements neither a hash nor a random generator
/// of its own.
pub fn new_id() -> String {
    /// Mints so far in this process. `Relaxed` is enough: the value only has to
    /// be distinct per mint, not ordered against anything else.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha256::new();
    hasher.update(process_entropy());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 version (4) and variant bits, so the id has the same textual
    // shape as the ids the service hands out.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// This process's 128 bits of id-minting entropy, taken ONCE from `std`'s
/// OS-seeded [`RandomState`] — the same keys a `HashMap` is randomised with —
/// and never recomputed, so every id this process mints is unpredictable to
/// anyone who has only seen the others.
fn process_entropy() -> &'static [u8; 16] {
    static ENTROPY: OnceLock<[u8; 16]> = OnceLock::new();
    ENTROPY.get_or_init(|| {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&RandomState::new().hash_one(0_u8).to_be_bytes());
        bytes[8..].copy_from_slice(&RandomState::new().hash_one(u64::MAX).to_be_bytes());
        bytes
    })
}

/// The ARN of a domain.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("domain/{domain_id}"))
}

/// The ARN of a field of a domain.
pub fn field_arn(region: &str, domain_id: &str, field_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/field/{field_id}"),
    )
}

/// The ARN of a template of a domain.
pub fn template_arn(region: &str, domain_id: &str, template_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/template/{template_id}"),
    )
}

/// The ARN of a layout of a domain.
pub fn layout_arn(region: &str, domain_id: &str, layout_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/layout/{layout_id}"),
    )
}

/// The ARN of a case of a domain.
pub fn case_arn(region: &str, domain_id: &str, case_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/case/{case_id}"),
    )
}

/// The ARN of a related item of a case: the item is a resource UNDER its case,
/// so its ARN extends that case's own ([`case_arn`]) rather than standing beside
/// it.
pub fn related_item_arn(
    region: &str,
    domain_id: &str,
    case_id: &str,
    related_item_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/case/{case_id}/related-item/{related_item_id}"),
    )
}

/// The `RelatedItemType` the content of an association belongs to: the member
/// of the input union the request filled in.
///
/// The model names the six members of `RelatedItemInputContent` after the six
/// values of `RelatedItemType`
/// (`smithy_query(query="shape",shape="RelatedItemInputContent,RelatedItemType")`),
/// so the content itself says which type it is, and a `type` that names another
/// one contradicts it.
pub fn related_item_content_type(content: &RelatedItemInputContent) -> &'static str {
    match content {
        RelatedItemInputContent::Contact(_) => "Contact",
        RelatedItemInputContent::Comment(_) => "Comment",
        RelatedItemInputContent::File(_) => "File",
        RelatedItemInputContent::Sla(_) => "Sla",
        RelatedItemInputContent::connectCase(_) => "ConnectCase",
        RelatedItemInputContent::Custom(_) => "Custom",
    }
}

/// A layout's `content` in the COMPLETE form the service keeps it in: a
/// `basic` layout has both of its panels, and each panel has a `sections` list
/// — empty where the request named no section — while everything the request
/// did spell (each section, its field group's name and the order of its fields)
/// is kept exactly as given.
///
/// The real service answers a panel the create never mentioned as present and
/// empty rather than absent, so the content is normalised ONCE here, on the
/// write, and every read reports the stored record rather than re-deriving it.
pub fn normalized_layout_content(content: LayoutContent) -> LayoutContent {
    let LayoutContent::Basic(basic) = content;
    LayoutContent::Basic(BasicLayout {
        top_panel: Some(normalized_layout_panel(basic.top_panel)),
        more_info: Some(normalized_layout_panel(basic.more_info)),
    })
}

/// One panel of [`normalized_layout_content`]: its sections, or none of them.
fn normalized_layout_panel(panel: Option<LayoutSections>) -> LayoutSections {
    LayoutSections {
        sections: Some(panel.and_then(|panel| panel.sections).unwrap_or_default()),
    }
}

/// Every field id a layout's `content` references, in document order:
/// `topPanel` first, then `moreInfo`, each panel's sections in the order they
/// are listed and each field group's fields in the order it lists them. One
/// pass over the content, so it is linear in the number of field items and
/// bounded by [`MAX_LAYOUT_SECTIONS`] and [`MAX_LAYOUT_SECTION_FIELDS`].
pub fn layout_field_ids(content: &LayoutContent) -> Vec<String> {
    let LayoutContent::Basic(basic) = content;
    [basic.top_panel.as_ref(), basic.more_info.as_ref()]
        .into_iter()
        .flatten()
        .flat_map(|panel| panel.sections.iter().flatten())
        .flat_map(|section| {
            let Section::fieldGroup(group) = section;
            group.fields.iter().map(|item| item.id.clone())
        })
        .collect()
}

/// A layout's `content` as a READ reports it: the stored record with every
/// field item whose id `is_deleted` accepts left out, and every panel, section
/// and field group otherwise exactly as stored, in the order stored.
///
/// A read of a layout "does not return field IDs for deleted fields"
/// (`smithy_query(query="doc",shape="DeleteField")`), so a deleted field is
/// filtered out on the read rather than edited out of the record: the write's
/// content is what stays stored, and a field whose delete is the only change
/// is the only difference a reader sees. One pass over the content, so it is
/// linear in the number of field items, as [`layout_field_ids`] is.
pub fn layout_content_without_deleted_fields(
    content: &LayoutContent,
    is_deleted: impl Fn(&str) -> bool,
) -> LayoutContent {
    let LayoutContent::Basic(basic) = content;
    LayoutContent::Basic(BasicLayout {
        top_panel: basic
            .top_panel
            .as_ref()
            .map(|panel| layout_panel_without_deleted_fields(panel, &is_deleted)),
        more_info: basic
            .more_info
            .as_ref()
            .map(|panel| layout_panel_without_deleted_fields(panel, &is_deleted)),
    })
}

/// One panel of [`layout_content_without_deleted_fields`]: its sections, each
/// field group holding every field item `is_deleted` rejects.
fn layout_panel_without_deleted_fields(
    panel: &LayoutSections,
    is_deleted: &impl Fn(&str) -> bool,
) -> LayoutSections {
    LayoutSections {
        sections: panel.sections.as_ref().map(|sections| {
            sections
                .iter()
                .map(|section| {
                    let Section::fieldGroup(group) = section;
                    Section::fieldGroup(FieldGroup {
                        name: group.name.clone(),
                        fields: group
                            .fields
                            .iter()
                            .filter(|item| !is_deleted(&item.id))
                            .cloned()
                            .collect(),
                    })
                })
                .collect()
        }),
    }
}

/// `Some(message)` when a layout's `content` carries more sections in one panel
/// than [`MAX_LAYOUT_SECTIONS`], or more fields in one group than
/// [`MAX_LAYOUT_SECTION_FIELDS`]: the refusal text naming the member, the
/// length seen and the ceiling.
///
/// Checked in the handler for the same reason as [`list_too_long`], which it
/// builds the message with: the per-request work is bounded by this code rather
/// than only by the boundary validator.
pub fn layout_content_too_long(content: &LayoutContent) -> Option<String> {
    let LayoutContent::Basic(basic) = content;
    for (panel_name, panel) in [
        ("content.basic.topPanel", basic.top_panel.as_ref()),
        ("content.basic.moreInfo", basic.more_info.as_ref()),
    ] {
        let sections = panel.and_then(|panel| panel.sections.as_deref()).unwrap_or(&[]);
        if let Some(message) = list_too_long(
            &format!("{panel_name}.sections"),
            sections.len(),
            MAX_LAYOUT_SECTIONS,
        ) {
            return Some(message);
        }
        for section in sections {
            let Section::fieldGroup(group) = section;
            if let Some(message) = list_too_long(
                &format!("{panel_name}.sections.fieldGroup.fields"),
                group.fields.len(),
                MAX_LAYOUT_SECTION_FIELDS,
            ) {
                return Some(message);
            }
        }
    }
    None
}

/// The region to build a new resource's ARN in: the one the caller signed for,
/// else [`DEFAULT_REGION`]. An ARN is minted once and stored, so a later read in
/// another region still reports the ARN the resource was created with.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A new domain's field schema: the reserved `System`-namespace fields
/// [`RESERVED_SYSTEM_FIELDS`] names, keyed by the reserved id each one is
/// addressed by.
///
/// A reserved field's `name` is the service's DISPLAY name for it, which the
/// catalog states beside the reserved id it is addressed by: the id is the
/// service's own word for the field and the name is what a human reads, and the
/// model keeps the two apart. Its ARN is built like any other field's, so it is
/// addressable in the domain's region, and it is created with the domain, so its
/// instants are the domain's.
pub fn reserved_system_fields(
    region: &str,
    domain_id: &str,
    created_time: SystemTime,
) -> HashMap<String, Field> {
    RESERVED_SYSTEM_FIELDS
        .iter()
        .map(|(field_id, name, field_type)| {
            (
                (*field_id).to_string(),
                Field {
                    field_id: (*field_id).to_string(),
                    field_arn: field_arn(region, domain_id, field_id),
                    name: (*name).to_string(),
                    field_type: (*field_type).to_string(),
                    namespace: FIELD_NAMESPACE_SYSTEM.to_string(),
                    description: None,
                    attributes: None,
                    deleted: false,
                    tags: HashMap::new(),
                    // A reserved field is provisioned with no options: only
                    // `BatchPutFieldOptions` puts them, and nothing has called
                    // it against a domain that has just been created.
                    options: BTreeMap::new(),
                    created_time,
                    last_modified_time: created_time,
                },
            )
        })
        .collect()
}

/// The reserved [`RESERVED_SYSTEM_FIELDS`] a LAYOUT may not place, by their
/// reserved ids. The model states this for the layout operations in particular:
/// "Title and Status fields cannot be part of layouts since they are not
/// configurable" (`smithy_query(query="doc",shape="CreateLayout")`), so a
/// layout naming one of them is refused even though the domain's schema does
/// hold it.
pub const FIELDS_NOT_CONFIGURABLE_IN_A_LAYOUT: &[&str] = &["title", "status"];

/// The ids among `ids` that a layout may not place, each reported once in
/// first-seen order. Empty means every referenced field is configurable.
/// Linear in the number of ids.
pub fn not_configurable_in_a_layout<'a, I: IntoIterator<Item = &'a str>>(ids: I) -> Vec<String> {
    let mut reported: HashSet<&str> = HashSet::new();
    let mut refused: Vec<String> = Vec::new();
    for id in ids {
        if FIELDS_NOT_CONFIGURABLE_IN_A_LAYOUT.contains(&id) && reported.insert(id) {
            refused.push(id.to_string());
        }
    }
    refused
}

/// The per-item error code a batch read reports for an id it cannot resolve.
///
/// A batch read partially succeeds — HTTP 200, with the ids it could not
/// resolve in its own `errors` list — so the code it reports per item is the
/// service's own per-item vocabulary, which the model leaves as a free string
/// (`smithy_query(query="shape",shape="FieldError")` targets `errorCode` at
/// `String` and enumerates nothing). It is NOT the name of a modelled
/// exception shape: `ResourceNotFoundException` is how a WHOLE call fails,
/// while one unresolvable item is `ResourceNotFound`.
pub const BATCH_ITEM_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The kind of resource a refusal is about: the kinds this service addresses
/// within a container, plus the container itself. Taking the kind as a value
/// rather than a string is what makes the rendering of [`name_conflict`] and
/// [`not_found`] total — there is no unknown kind to fall back on, and no
/// runtime capitalisation: each kind spells itself once, so the `resourceType`
/// token a refusal reports is the service's own lowercase word for the kind and
/// the sentence of a refusal is chosen deliberately ([`NotFoundWording`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Domain,
    Field,
    Template,
    Layout,
    Case,
}

impl ResourceKind {
    /// The kind as the conflict template spells it: `(leading, inline)`.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::Domain => ("Domain", "domain"),
            Self::Field => ("Field", "field"),
            Self::Template => ("Template", "template"),
            Self::Layout => ("Layout", "layout"),
            Self::Case => ("Case", "case"),
        }
    }

    /// The `resourceType` a refusal about this kind reports: the service's
    /// lowercase token for the kind, which is the wire vocabulary a client
    /// branches on to tell WHICH identifier of a request could not be resolved
    /// — the same token the kind reads as inside a sentence, never a
    /// capitalised type name.
    pub fn resource_type(self) -> &'static str {
        self.words().1
    }
}

/// Which sentence a [`not_found`] refusal carries.
///
/// The service does not word every not-found refusal alike, and the wording is
/// not a function of the resource class alone: a call that ADDRESSES a case
/// quotes the case id it was given, while the association of a related item
/// answers a fixed sentence about the same class, so the sentence belongs to
/// the refusal — the class, and the operation raising it where that operation
/// words the class differently. A raising site therefore names its wording, and
/// [`ResourceKind`] decides the `resourceType` token only. The refusals this
/// service words no sentence of its own for carry [`Self::Generic`], which the
/// recorded layout refusal pins as a wording this service really sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFoundWording {
    /// The service's generic sentence, with no variable part
    /// ([`RESOURCE_NOT_FOUND_MESSAGE`]): the resource is reported through the
    /// error's own `resourceId`/`resourceType` members instead.
    Generic,
    /// What a call that addresses a case by its `caseId` path label, or by a
    /// case reference in its body, answers for a case the domain does not hold:
    /// the sentence quotes that id.
    AddressedCase,
    /// What the association of a related item answers for the case it is about:
    /// entirely fixed text, quoting no id.
    RelatedItemCase,
    /// What a create of a case in a domain answers for the domain itself: the
    /// sentence quotes the domain identifier the request carried.
    NamedDomain,
}

impl NotFoundWording {
    /// This wording's sentence for a refusal about `resource_id`, the only
    /// value a sentence of this service ever interpolates.
    pub fn sentence(self, resource_id: &str) -> String {
        match self {
            Self::Generic => RESOURCE_NOT_FOUND_MESSAGE.to_string(),
            Self::AddressedCase => format!("A case with id '{resource_id}' could not be found."),
            Self::RelatedItemCase => RELATED_ITEM_CASE_NOT_FOUND_MESSAGE.to_string(),
            Self::NamedDomain => format!("Domain '{resource_id}' does not exist."),
        }
    }
}

/// The modelled 409 for a create or a rename refused because a live resource of
/// the same kind in the same container already holds the requested name.
///
/// The message is rendered from the REQUEST's own name with the service's fixed
/// template — `<Kind> creation failed: <kind> resource with the name <name>
/// already exists!` — so a client that matches on it reads back only what it
/// sent, never a server-minted id of the resource that won the name (which the
/// loser never saw and cannot match on).
///
/// `CreateField`'s rendering is the one the recorded traffic pins; the other
/// kinds are the same rule applied to the other creates that refuse on a taken
/// name, so the refusals read alike instead of each inventing a sentence. A
/// RENAME reuses the same sentence deliberately: it is the same rule, about the
/// same clash, and nothing records a second wording for it — should a recording
/// show the verb tracking the caller, that is the one place to change.
pub fn name_conflict(resource_kind: ResourceKind, name: &str) -> ConflictException {
    let (leading, inline) = resource_kind.words();
    ConflictException {
        message: format!(
            "{leading} creation failed: {inline} resource with the name {name} already exists!"
        ),
    }
}

/// The modelled 404 for a resource of `resource_kind` this service does not
/// hold, addressed by `resource_id`.
///
/// The `resourceId` and `resourceType` members the model makes required
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`) always
/// report what could not be resolved — the kind through its lowercase wire
/// token ([`ResourceKind::resource_type`]) — and the `message` is the
/// [`NotFoundWording::Generic`] sentence: the refusals this service words a
/// sentence of their own for raise [`not_found_worded`] instead.
pub fn not_found(resource_kind: ResourceKind, resource_id: &str) -> ResourceNotFoundException {
    not_found_worded(resource_kind, resource_id, NotFoundWording::Generic)
}

/// The modelled 404 of [`not_found`], worded as `wording` words it: what a
/// refusal the service sends a sentence of its own for raises.
pub fn not_found_worded(
    resource_kind: ResourceKind,
    resource_id: &str,
    wording: NotFoundWording,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: wording.sentence(resource_id),
        resource_id: resource_id.to_string(),
        resource_type: resource_kind.resource_type().to_string(),
    }
}

/// The `message` a [`not_found`] carries for a kind of resource the service has
/// no sentence of its own for: its generic sentence, with no variable part.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Requested resource not found.";

/// The sentence the association of a related item carries for a case the domain
/// does not hold: fixed text with no variable part, quoting no id — the case is
/// reported through the error's `resourceId` instead.
pub const RELATED_ITEM_CASE_NOT_FOUND_MESSAGE: &str = "A case with the specified ID was not found.";

/// The modelled 500 for a request that reached a handler with no store behind
/// it. Answered rather than papered over with a private engine: a handler that
/// builds its own store writes somewhere nothing else can read.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the ConnectCases state store is unavailable".to_string(),
        retry_after_seconds: None,
    }
}

/// Render a stored instant the way the model's `date-time` timestamps expect.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The case-event publishing configuration a domain reports when no
/// `PutCaseEventConfiguration` has been made against it: delivery disabled, with
/// no included data.
///
/// The configuration is a PROPERTY of the domain, not a resource of its own:
/// `PutCaseEventConfiguration` "Adds case event publishing configuration" to a
/// domain and `GetCaseEventConfiguration` "Returns the case event publishing
/// configuration" with `eventBridge` as a REQUIRED output member
/// (`smithy_query(query="operation", operation="GetCaseEventConfiguration")`), so
/// an unconfigured domain reports event delivery off rather than a missing
/// resource — the 404 of either operation is for a domain this store does not
/// hold.
pub fn event_bridge_disabled() -> EventBridgeConfiguration {
    EventBridgeConfiguration {
        enabled: false,
        included_data: None,
    }
}

/// How many field identifiers `configuration` asks the case event stream to
/// include: the length of its `includedData.caseData.fields` list, or zero when
/// it names no case data at all. Linear in nothing — it reads one length.
pub fn included_field_count(configuration: &EventBridgeConfiguration) -> usize {
    configuration
        .included_data
        .as_ref()
        .and_then(|included| included.case_data.as_ref())
        .map_or(0, |case_data| case_data.fields.len())
}

/// A tag map for a response member: absent when the resource carries no tags,
/// which is how the service reports one that was never tagged.
pub fn tags_output(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(tags.clone())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcasesAmazonconnectcasesState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcasesAmazonconnectcasesState>)
}
