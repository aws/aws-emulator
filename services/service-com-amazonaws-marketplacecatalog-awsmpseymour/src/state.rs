// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacecatalog-awsmpseymour service.
//!
//! The change set is the primary resource of the AWS Marketplace Catalog:
//! `StartChangeSet` mints one, `DescribeChangeSet` / `ListChangeSets` report it
//! and `CancelChangeSet` stops one that is still running. A change set that
//! succeeds with `Intent=APPLY` is what makes a catalog ENTITY exist, and
//! `DescribeEntity` / `ListEntities` report those entities. The two records
//! below are the one copy of each that every handler of this crate reads and
//! writes, plus the projections the model asks for (`ChangeSummary`,
//! `ChangeSetSummaryListItem`, `EntitySummary`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::Document;
use serde_json::{Map, Value};

use crate::ops::types::{
    AccessDeniedException, AMISecuritySummary, AmiProductSummary, AssessmentSummary,
    AssessmentTargetFilter, AssessmentTargetSummary, ChangeSetSummaryListItem, ChangeSummary,
    CHANGE_STATUS_VALUES,
    ContainerProductSummary, ContainerSecuritySummary, ControlAssessment, ControlError,
    ControlSummary, DataProductSummary, Entity, EntitySummary, ErrorDetail, ErrorScope,
    ExpressPrivateOfferConfigurationSummary, Filter, FrameworkFilters, FrameworkSummary,
    MachineLearningProductSummary, OfferSetSummary, OfferSummary, ResaleAuthorizationSummary,
    ResourceInUseException, ResourceNotFoundException, SaaSProductSummary,
    SubsidiaryAuthorizationSummary, Tag,
    ValidationException, AMI_PRODUCT_SORT_BY_VALUES, AMI_PRODUCT_VISIBILITY_STRING_VALUES,
    CONTAINER_PRODUCT_SORT_BY_VALUES, CONTAINER_PRODUCT_VISIBILITY_STRING_VALUES,
    DATA_PRODUCT_SORT_BY_VALUES, DATA_PRODUCT_VISIBILITY_STRING_VALUES,
    EXPRESS_PRIVATE_OFFER_CONFIGURATION_SORT_BY_VALUES, MACHINE_LEARNING_PRODUCT_SORT_BY_VALUES,
    MACHINE_LEARNING_PRODUCT_VISIBILITY_STRING_VALUES, OFFER_SET_SORT_BY_VALUES,
    OFFER_SORT_BY_VALUES, RESALE_AUTHORIZATION_SORT_BY_VALUES, SAA_S_PRODUCT_SORT_BY_VALUES,
    SAA_S_PRODUCT_VISIBILITY_STRING_VALUES, SUBSIDIARY_AUTHORIZATION_SORT_BY_VALUES,
};
mod assessment_controls;
pub use assessment_controls::*;

/// The ARNs this service mints, and the identifiers that address a resource
/// through one.
mod arns;
pub use arns::*;

/// The only catalog this service serves.
///
/// Every operation's `Catalog` member documents "Fixed value: AWSMarketplace",
/// so a request naming any other catalog is refused rather than opening a
/// second store.
pub const AWS_MARKETPLACE_CATALOG: &str = "AWSMarketplace";

/// The region an ARN names when the request carries no SigV4 credential scope
/// to read one from, or names a region no ARN this service mints can carry
/// ([`is_arn_region`]).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` of this service, and so the service segment of every ARN
/// it mints (`OPERATIONS.md`'s wire-protocol section).
pub const ARN_NAMESPACE: &str = "aws-marketplace";

/// How long a change set stays `PREPARING` before the clock owes it `APPLYING`.
///
/// A change set is asynchronous: `StartChangeSet` returns before the changes
/// have run. AWS takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero, so a caller acting straight after the start still
/// observes the running change set — and can still cancel it. The due instant
/// is stored on the record and materialised by whichever later call reads it;
/// nothing sleeps.
pub const CHANGE_SET_PREPARING: Duration = Duration::from_millis(250);

/// How long a change set then stays `APPLYING` before the clock owes it
/// `SUCCEEDED` — and, for `Intent=APPLY`, owes the catalog the entities its
/// changes describe. Chosen for the same reason as [`CHANGE_SET_PREPARING`].
pub const CHANGE_SET_APPLYING: Duration = Duration::from_millis(250);

/// How many change sets the store holds before `StartChangeSet` refuses with
/// the modelled `ServiceQuotaExceededException` ("The maximum number of open
/// requests per account has been exceeded").
///
/// AWS publishes an adjustable quota on change sets per account, which this
/// emulator models neither Service Quotas nor an account's adjustments for — so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is far above any workflow an emulator serves, and it is
/// what stops the state growing without end.
pub const CHANGE_SET_QUOTA: usize = 1_000;

/// The largest page `ListChangeSets` serves: `ListChangeSetsRequest$MaxResults`
/// is `@range(min: 1, max: 20)`.
pub const LIST_CHANGE_SETS_PAGE_LIMIT: i32 = 20;

/// The page `ListChangeSets` serves when the request names no `MaxResults`:
/// `ListChangeSetsRequest$MaxResults` documents "By default, this value is 20".
/// The member carries no `@default`, so this handler applies it; here it happens
/// to coincide with [`LIST_CHANGE_SETS_PAGE_LIMIT`].
pub const LIST_CHANGE_SETS_PAGE_DEFAULT: i32 = 20;

/// The largest page `ListEntities` serves: `ListEntitiesRequest$MaxResults` is
/// `@range(min: 1, max: 50)`.
pub const LIST_ENTITIES_PAGE_LIMIT: i32 = 50;

/// The page `ListEntities` serves when the request names no `MaxResults`:
/// `ListEntitiesRequest$MaxResults` documents "Specifies the upper limit of the
/// elements on a single page. If a value isn't provided, the default value is
/// 20". The member carries no `@default`, so this handler applies it — and it is
/// well below [`LIST_ENTITIES_PAGE_LIMIT`], the largest page a request may ASK
/// for.
pub const LIST_ENTITIES_PAGE_DEFAULT: i32 = 20;

/// The `Filter$Name` values `ListChangeSets` accepts, from `Filter$ValueList`'s
/// own documentation ("The supported filter names and associated `ValueList`s is
/// as follows: ChangeSetName, Status, EntityId, BeforeStartTime, AfterStartTime,
/// BeforeEndTime, AfterEndTime").
pub const CHANGE_SET_FILTER_NAMES: &[&str] = &[
    "AfterEndTime",
    "AfterStartTime",
    "BeforeEndTime",
    "BeforeStartTime",
    CHANGE_SET_NAME_FILTER_NAME,
    ENTITY_ID_FILTER_NAME,
    CHANGE_SET_STATUS_FILTER_NAME,
];

/// The `Filter$Name` values `ListEntities` accepts: `Filter$Name` documents
/// "For `ListEntities`, the supported value for this is an `EntityId`".
pub const ENTITY_FILTER_NAMES: &[&str] = &[ENTITY_ID_FILTER_NAME];

/// The `Filter$Name` that selects on the id of an entity, which both lists
/// filter on: `Filter$Name` names it the one attribute `ListEntities` filters
/// on, and `Filter$ValueList` lists it among the attributes `ListChangeSets`
/// does.
pub const ENTITY_ID_FILTER_NAME: &str = "EntityId";

/// The `Filter$Name` that selects `ListChangeSets` on the name a change set was
/// started with (`Filter$ValueList`: "`ChangeSetName` - ... These are defined
/// when you call the `StartChangeSet` action").
pub const CHANGE_SET_NAME_FILTER_NAME: &str = "ChangeSetName";

/// The name a change set started WITHOUT a `ChangeSetName` carries: the
/// `ChangeSetName` members of `DescribeChangeSetResponse` and
/// `ChangeSetSummaryListItem` are documented "the optional name provided in the
/// `StartChangeSet` request. If you do not provide a name, one is set by
/// default", so the service — not the caller — names an unnamed change set, and
/// that name is reported and filtered on like a submitted one.
///
/// The name set by default is the change set's own identifier: it is already
/// unique over the store, it never changes for the life of the record, and
/// `ResourceId`'s `^[\w\-]+$` is a subset of `ChangeSetName`'s
/// `^[\w\s+=.:@-]+$` within its 1..100 length, so the default satisfies the
/// member's own contract ([`is_change_set_name`]).
pub fn default_change_set_name(change_set_id: &str) -> String {
    change_set_id.to_string()
}

/// The `ListChangeSets` filters whose `ValueList` is a single instant rather
/// than a set of values to match: each one is documented as "all change sets
/// that started/ended before/after THE filter value".
pub const CHANGE_SET_TIME_FILTER_NAMES: &[&str] = &[
    "AfterEndTime",
    "AfterStartTime",
    "BeforeEndTime",
    "BeforeStartTime",
];

/// The `Filter$Name` whose `ValueList` entries name members of a set the model
/// ENUMERATES rather than values a caller invents: `Filter$ValueList` documents
/// it as "a list of statuses for all change set requests", and the statuses a
/// change set request has are the members of `ChangeStatus`
/// ([`CHANGE_STATUS_VALUES`]).
pub const CHANGE_SET_STATUS_FILTER_NAME: &str = "Status";

/// The closed set a `Filter$ValueList` entry of the filter `name` has to name,
/// and the noun a refusal calls one of its members by — and `None` for a filter
/// whose values are the names and ids a caller chose (a change set name, an
/// entity id), which no published set bounds.
///
/// A filter selects on an attribute, so when the model enumerates that
/// attribute's values, a filter value outside that enum names no value the
/// attribute can hold — refused, as every other value of this service naming
/// nothing the catalog has is refused, rather than silently selecting the empty
/// page it would match.
pub fn filter_value_set(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    match name {
        CHANGE_SET_STATUS_FILTER_NAME => Some(("status", CHANGE_STATUS_VALUES)),
        _ => None,
    }
}

/// The FORM the values of one filter attribute take: the predicate that judges a
/// value and the refusal built for one outside it.
#[derive(Debug, Clone, Copy)]
pub struct ValueForm {
    /// Whether a value is of this form.
    pub well_formed: fn(&str) -> bool,
    /// The modelled `ValidationException` for a value outside it, built from the
    /// value — the same fault the member carrying such a value answers
    /// elsewhere in this service.
    pub refusal: fn(&str) -> ValidationException,
}

/// The FORM a `Filter$ValueList` entry of the filter `name` has to take, and
/// `None` for a filter whose values this service states no form for.
///
/// A filter selects on an attribute, so a value that the attribute's own values
/// can never be names nothing the attribute holds. This service states the form
/// of each kind of value it keeps ([`is_entity_id`], [`is_change_set_name`]) and
/// refuses a member carrying one outside it wherever such a member appears, so
/// the same decision is made on a filter value: it is refused with that form's
/// own `ValidationException`, quoting the value and the syntax it had to
/// satisfy, rather than silently selecting the empty page it would match.
///
/// [`filter_value_set`] is the other half of the same question: a closed SET
/// bounds the attributes the model enumerates, and a stated FORM bounds the ids
/// and names a caller chooses.
pub fn filter_value_form(name: &str) -> Option<ValueForm> {
    match name {
        ENTITY_ID_FILTER_NAME => Some(ValueForm {
            well_formed: is_entity_id,
            refusal: invalid_entity_id,
        }),
        CHANGE_SET_NAME_FILTER_NAME => Some(ValueForm {
            well_formed: is_change_set_name,
            refusal: invalid_change_set_name,
        }),
        _ => None,
    }
}

/// Whether the `ValueList` of the filter `name` is a list of UNIQUE values.
///
/// `Filter$ValueList` states it per filter: the `EntityId` list is "a list of
/// unique `EntityId`s" while the `ChangeSetName` list is "a list of NON-unique
/// `ChangeSetName`s", so a value repeated in the first is a request the
/// documentation forbids and one repeated in the second is not. A filter the
/// documentation says nothing about is left unconstrained.
pub fn filter_values_unique(name: &str) -> bool {
    name == ENTITY_ID_FILTER_NAME
}

/// The first entry of `values` that an earlier entry already carries, and `None`
/// when every entry is distinct.
///
/// A `ValueList` is `@length(min: 1, max: 10)`, so the pairwise comparison is
/// over a list the model already bounds.
fn first_repeated(values: &[String]) -> Option<&String> {
    values
        .iter()
        .enumerate()
        .find(|(at, value)| values[..*at].contains(value))
        .map(|(_, value)| value)
}

/// Whether the `ValueList` of the field `member` of the `EntityTypeFilters`
/// shape belonging to `entity_type` is a list of UNIQUE values.
///
/// The per-type filter shapes state it per field, exactly as `Filter$ValueList`
/// does for the generic filter surface ([`filter_values_unique`]): the four
/// product types whose filter fields are documented "A string array of unique
/// entity id / product title / visibility values to be filtered on"
/// (`AmiProductEntityIdFilter$ValueList`, `AmiProductTitleFilter$ValueList`,
/// `AmiProductVisibilityFilter$ValueList` and the same three shapes of
/// `ContainerProduct`, `DataProduct` and `SaaSProduct` —
/// `smithy_query(query="shape", shape="AmiProductEntityIdFilter")`) state those
/// three lists unique, so a value repeated in one of them is a request the
/// documentation forbids.
///
/// Every other `*Filters` field's `ValueList` states no uniqueness — the
/// `MachineLearningProduct` fields are documented "A list of entity IDs to
/// filter by", and the `Offer`, `OfferSet`, `ResaleAuthorization` and
/// `ExpressPrivateOfferConfiguration` fields "with list input" / "A list of
/// `ProductId` values to filter on" — so, as on the generic surface, a field the
/// documentation says nothing about is left unconstrained. No `ValueList` shape
/// in the model carries `@uniqueItems`, so this prose is the whole of the rule.
pub fn type_filter_values_unique(entity_type: &str, member: &str) -> bool {
    match entity_type {
        "AmiProduct" | "ContainerProduct" | "DataProduct" | "SaaSProduct" => matches!(
            member,
            ENTITY_ID_FILTER_NAME | "ProductTitle" | ENTITY_VISIBILITY_DETAIL_MEMBER
        ),
        _ => false,
    }
}

/// The modelled `ValidationException` for a value `values` names twice, and
/// `None` when every value in it is distinct.
///
/// This is the one place a repeated filter value is refused, so both filter
/// surfaces of a list operation — the generic `FilterList` and a type's own
/// `*Filters` — refuse the same repetition the same way: `field` is the request
/// member the list arrived in and `filter` the filter it belongs to.
pub fn repeated_filter_value(
    field: &str,
    filter: &str,
    values: &[String],
) -> Option<ValidationException> {
    first_repeated(values).map(|repeated| {
        validation(
            field,
            quoting(
                &format!("filter {filter} matches a list of unique values, and '"),
                repeated,
                "' is named twice",
            ),
        )
    })
}

/// The `Sort$SortBy` values `ListChangeSets` accepts, from `Sort$SortBy`'s doc
/// ("For `ListChangeSets`, supported attributes include `StartTime` and
/// `EndTime`"). The first entry is the default.
pub const CHANGE_SET_SORT_BY: &[&str] = &["StartTime", "EndTime"];

/// The `Sort$SortBy` values `ListEntities` accepts, from the same doc ("For
/// `ListEntities`, supported attributes include `LastModifiedDate` (default) and
/// `EntityId`"). The first entry is the default.
pub const ENTITY_SORT_BY: &[&str] = &["LastModifiedDate", "EntityId"];

/// The `SortOrder` value that orders a page from the greatest sort key down —
/// the enum's descending member, one of the two values `SortOrder` accepts
/// (`SORT_ORDER_VALUES` in `crate::ops::types`).
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The `SortOrder` a list uses when the request names none: `Sort$SortOrder`
/// documents "The default value is `DESCENDING`".
pub const DEFAULT_SORT_ORDER: &str = SORT_ORDER_DESCENDING;

/// Whether `sort_order` reads a page from the greatest sort key down.
///
/// `Sort$SortOrder` documents "The sorting order. Can be `ASCENDING` or
/// `DESCENDING`" — so exactly the descending member reverses the ordering and
/// every other value leaves it ascending. This is the one place that decision
/// is made, so each list operation turns a `SortOrder` into a direction the
/// same way, whatever order the default happens to be.
pub fn sorts_descending(sort_order: &str) -> bool {
    sort_order == SORT_ORDER_DESCENDING
}

/// The attribute one `Filter` selects on and the values it matches, or the
/// modelled `ValidationException` that refuses the filter.
///
/// `Filter$Name` is "the name of the filter, which is unique for each filter
/// that you want to apply" and names an attribute the list being called filters
/// on, while `Filter$ValueList` is "the list of values" that filter matches — so
/// a filter naming no attribute, one naming an attribute this list does not
/// filter on, and one carrying no value to match are three ways a filter is
/// refused, each under the member it is about. The fourth is a value the
/// attribute cannot hold, in either of the two ways this service bounds one:
/// where the model enumerates that attribute's values ([`filter_value_set`]), a
/// value outside the published set is refused, and where the service states the
/// FORM those values take ([`filter_value_form`]), a value outside that form is
/// refused with the same fault the member carrying such a value answers
/// anywhere else. The fifth is a value repeated in a `ValueList` the
/// documentation states to be a list of unique values ([`filter_values_unique`]).
/// This is the one place those decisions are made, so every list operation
/// judges the same `Filter` shape the same way: `operation` is the list being
/// called and `filtered_on` the attributes it publishes.
pub fn filter_predicate(
    operation: &str,
    filtered_on: &[&str],
    filter: Filter,
) -> Result<(String, Vec<String>), ValidationException> {
    let Some(name) = filter.name else {
        return Err(validation(
            "FilterList.Name",
            "every filter names an attribute",
        ));
    };
    if !filtered_on.contains(&name.as_str()) {
        return Err(validation(
            "FilterList.Name",
            format!("{operation} filters on {filtered_on:?}, not on {name:?}"),
        ));
    }
    let values = filter.value_list.unwrap_or_default();
    if values.is_empty() {
        return Err(validation(
            "FilterList.ValueList",
            format!("filter {name} carries no value to match"),
        ));
    }
    if let Some((noun, published)) = filter_value_set(&name) {
        if let Some(outside) = values
            .iter()
            .find(|value| !published.contains(&value.as_str()))
        {
            return Err(invalid_choice(noun, outside, published));
        }
    }
    if let Some(form) = filter_value_form(&name) {
        if let Some(malformed) = values.iter().find(|value| !(form.well_formed)(value)) {
            return Err((form.refusal)(malformed));
        }
    }
    if filter_values_unique(&name) {
        if let Some(fault) = repeated_filter_value("FilterList.ValueList", &name, &values) {
            return Err(fault);
        }
    }
    Ok((name, values))
}

/// The attributes a whole `FilterList` selects on and the values each matches,
/// or the modelled `ValidationException` that refuses the list.
///
/// Every filter is judged by [`filter_predicate`], and the list as a whole by
/// the one rule that is about the list rather than one filter: `Filter$Name` is
/// "the name of the filter, which is unique for each filter that you want to
/// apply", so a `FilterList` naming one attribute twice is a request the
/// documentation forbids — refused rather than accepted as two predicates
/// conjoined into a page neither filter asked for. This is the one place a list
/// operation turns its `FilterList` into predicates, so every list refuses the
/// same list the same way.
pub fn filter_predicates(
    operation: &str,
    filtered_on: &[&str],
    filters: Vec<Filter>,
) -> Result<Vec<(String, Vec<String>)>, ValidationException> {
    let mut predicates: Vec<(String, Vec<String>)> = Vec::with_capacity(filters.len());
    for filter in filters {
        let (name, values) = filter_predicate(operation, filtered_on, filter)?;
        if predicates.iter().any(|(applied, _)| applied == &name) {
            return Err(validation(
                "FilterList.Name",
                format!(
                    "filter name '{name}' is named twice, and a filter name is unique for \
                     each filter a request applies"
                ),
            ));
        }
        predicates.push((name, values));
    }
    Ok(predicates)
}

/// The details member the five product summaries name an entity's `Name`:
/// `AmiProductSummary$ProductTitle` and the four product titles beside it.
pub const PRODUCT_TITLE_DETAIL_MEMBER: &str = "ProductTitle";

/// The details member every other type's summary names an entity's `Name`:
/// `OfferSummary$Name`, `OfferSetSummary$Name`,
/// `ResaleAuthorizationSummary$Name`.
pub const NAME_DETAIL_MEMBER: &str = "Name";

/// The details members an entity's `Name` is read from, across every type this
/// catalog has.
///
/// `EntitySummary$Name` is "defined by the seller", and the per-type summaries
/// the model carries name it two ways: the five product summaries
/// (`AmiProductSummary`, `ContainerProductSummary`, `DataProductSummary`,
/// `SaaSProductSummary`, `MachineLearningProductSummary`) call it
/// `ProductTitle`, while `OfferSummary` / `OfferSetSummary` /
/// `ResaleAuthorizationSummary` call it `Name`.
///
/// This is their UNION — the members a change's details may carry a value an
/// entity's name is read from at, which is what the write path judges against
/// the name contract. WHICH of them ONE entity's name comes from follows that
/// entity's own type: [`entity_name_detail_member`].
pub const ENTITY_NAME_DETAIL_MEMBERS: &[&str] =
    &[PRODUCT_TITLE_DETAIL_MEMBER, NAME_DETAIL_MEMBER];

/// The ONE details member an entity of `entity_type` carries its `Name` in.
///
/// A type whose own summary names the entity's name names it under exactly one
/// member, and the two sets are disjoint: `ProductTitle` is no member of
/// `OfferSummary` / `OfferSetSummary` / `ResaleAuthorizationSummary`, and `Name`
/// is no member of any `*ProductSummary` (`smithy_query(query="shape",
/// shape="AmiProductSummary,OfferSummary")`). So the member an entity's name is
/// read from is the one ITS type names — `ProductTitle` for a product type
/// ([`PRODUCT_ENTITY_TYPES`]), `Name` for every other type, which is the member
/// a type whose summary names neither (`ExpressPrivateOfferConfiguration`, and
/// the types the model declares no summary for) carries it in as well, since
/// `EntitySummary$Name` is the only member reporting it there.
///
/// One member per type is what keeps the value the summaries REPORT and the
/// attribute the per-type filters and sorts select and order on
/// ([`EntityRecord::attribute_values`], which reads the details member of the
/// name the filter and sort shapes give it) the same value.
pub fn entity_name_detail_member(entity_type: &str) -> &'static str {
    if PRODUCT_ENTITY_TYPES.contains(&entity_type) {
        PRODUCT_TITLE_DETAIL_MEMBER
    } else {
        NAME_DETAIL_MEMBER
    }
}

/// The details member an entity's `Visibility` is read from — the name every
/// per-type summary and `*VisibilityFilter` in the model gives it.
pub const ENTITY_VISIBILITY_DETAIL_MEMBER: &str = "Visibility";

/// The `Visibility` values an entity of a type the model declares no visibility
/// enum for may carry: the union of the model's five `*VisibilityString` enums
/// (`AmiProductVisibilityString`, `ContainerProductVisibilityString`,
/// `DataProductVisibilityString`, `MachineLearningProductVisibilityString`,
/// `SaaSProductVisibilityString`).
///
/// A type that HAS such an enum is bounded by that enum alone and never by this
/// union — see [`entity_type_visibility_values`], which is what every read and
/// write of an entity's `Visibility` goes through.
pub const ENTITY_VISIBILITY_VALUES: &[&str] =
    &["Draft", "Limited", "Public", "Restricted", "Unavailable"];

/// The `Visibility` an entity carries when the change that created it named
/// none.
///
/// `EntitySummary$Visibility` documents `Limited` as "the entity is visible to
/// limited accounts only" — which is what an entity a change set has just
/// created is, since publishing it to buyers takes a further change.
pub const DEFAULT_ENTITY_VISIBILITY: &str = "Limited";

/// The entity types whose per-type summary is the pair every `*ProductSummary`
/// shape carries.
///
/// Taken from `EntitySummary`'s own members: the five `*ProductSummary` shapes
/// (`AmiProductSummary`, `ContainerProductSummary`, `DataProductSummary`,
/// `MachineLearningProductSummary`, `SaaSProductSummary`) carry exactly
/// `{ProductTitle, Visibility}`. The remaining types' summaries
/// (`OfferSummary`, `OfferSetSummary`, `ResaleAuthorizationSummary`,
/// `ExpressPrivateOfferConfigurationSummary`) name further members, each read
/// from the details member of that name — see [`EntityRecord::to_summary`].
pub const PRODUCT_ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "MachineLearningProduct",
    "SaaSProduct",
];

/// The entity types whose own per-type shapes the model declares: the types a
/// request can filter, sort and summarise field by field.
///
/// Read off `EntitySummary`'s per-type summary members (`AmiProductSummary`,
/// `ContainerProductSummary`, `DataProductSummary`, `SaaSProductSummary`,
/// `MachineLearningProductSummary`, `OfferSummary`,
/// `ResaleAuthorizationSummary`, `OfferSetSummary`,
/// `ExpressPrivateOfferConfigurationSummary`) with the suffix dropped, which is
/// the same set the `EntityTypeFilters` / `EntityTypeSort` unions name
/// (`AmiProductFilters`, `AmiProductSort`, …). A type OUTSIDE this set is still
/// a type this catalog has ([`ENTITY_TYPES`]) — it just carries no per-type
/// summary, filter or sort shape, so it is summarised by the members every
/// entity carries and ordered only by [`ENTITY_SORT_BY`].
pub const SHAPED_ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "ExpressPrivateOfferConfiguration",
    "MachineLearningProduct",
    "Offer",
    "OfferSet",
    "ResaleAuthorization",
    "SaaSProduct",
    "SubsidiaryAuthorization",
];

/// The entity types this catalog has at all — what an `EntityType` member is
/// checked against, beyond the characters `EntityType`'s `@pattern` admits.
///
/// `EntityType` is `^[a-zA-Z]+$`, which admits any letters-only word; the
/// catalog itself serves a FIXED set of types, and the model names that set in
/// two places, each naming types the other leaves out, so the set is their
/// UNION:
///
/// * `ListEntitiesRequest$EntityType` enumerates them in prose — "Valid values
///   are: `AmiProduct`, `ContainerProduct`, `DataProduct`, `SaaSProduct`,
///   `ProcurementPolicy`, `Experience`, `Audience`, `BrandingSettings`,
///   `Offer`, `OfferSet`, `Seller`, `ResaleAuthorization`, `Solution`,
///   `SubsidiaryAuthorization`"
///   (`smithy_query(query="doc", shape="ListEntitiesRequest",
///   member="EntityType")`);
/// * the per-type shapes the model declares name the rest
///   ([`SHAPED_ENTITY_TYPES`], which adds `MachineLearningProduct` and
///   `ExpressPrivateOfferConfiguration`).
///
/// A type the model names either way is one this catalog serves, so listing it
/// answers a page (empty while the catalog holds nothing of it) and a change set
/// may create one. A letters-only value naming NO type of the model names no
/// entity type at all, so it is refused rather than listed as empty.
pub const ENTITY_TYPES: &[&str] = &[
    "AmiProduct",
    "Audience",
    "BrandingSettings",
    "ContainerProduct",
    "DataProduct",
    "Experience",
    "ExpressPrivateOfferConfiguration",
    "MachineLearningProduct",
    "Offer",
    "OfferSet",
    "ProcurementPolicy",
    "ResaleAuthorization",
    "SaaSProduct",
    "Seller",
    "Solution",
    "SubsidiaryAuthorization",
];

/// The change types this catalog performs — what a `Change$ChangeType` is
/// checked against, beyond the characters `ChangeType`'s `@pattern` admits.
///
/// `ChangeType` is "single string values that describe your intention for the
/// change. Each change type is unique for each `EntityType` provided in the
/// change's scope", and the model constrains only their syntax
/// (`^[A-Z][\w]*$`, `smithy_query(query="shape", shape="Change")`), pointing at
/// the reference pages for each kind of entity ("Working with single-AMI
/// products", "Working with container products", and the offer and resale
/// authorization pages beside them) for the values themselves. Those pages are
/// where the catalogue below is read off.
///
/// It is read as ONE set — the change types this catalog performs — rather than
/// as a change type per entity type: the pages publish a set per PRODUCT and
/// name none at all for several of the types this catalog serves
/// ([`ENTITY_TYPES`] names fifteen), so a per-type pairing is nowhere this
/// service states it. What it does state either way is that a change type is the
/// name of
/// an intention the catalog can carry out, so a value naming none of them asks
/// for no change this catalog can make, and is refused rather than stored as a
/// change nothing could apply.
pub const CHANGE_TYPES: &[&str] = &[
    "AddDeliveryOptions",
    "AddDimensions",
    "AddInstanceTypes",
    "AddRevisions",
    "ChangeVisibility",
    "CreateOffer",
    "CreateProduct",
    "CreateResaleAuthorization",
    "ReleaseOffer",
    "ReleaseProduct",
    "ReleaseResaleAuthorization",
    "RestrictDeliveryOptions",
    "RestrictDimensions",
    "RestrictInstanceTypes",
    "RestrictOffer",
    "RestrictProduct",
    "RestrictResaleAuthorization",
    "RestrictRevisions",
    "UpdateAvailability",
    "UpdateDeliveryOptions",
    "UpdateDimensions",
    "UpdateInformation",
    "UpdateLegalTerms",
    "UpdatePricingTerms",
    "UpdateRenewalTerms",
    "UpdateSupportTerms",
    "UpdateTargeting",
    "UpdateValidityTerms",
];

/// The resource type segment a CHANGE SET's ARN carries, as
/// [`change_set_arn`] writes it. An entity's ARN carries its `EntityType`
/// there instead, which is how one resource ARN form names both.
pub const CHANGE_SET_RESOURCE_TYPE: &str = "ChangeSet";

/// The policy document a resource carrying no resource-based policy reports.
///
/// `GetResourcePolicy` reports the policy OF a resource in a member targeting
/// `ResourcePolicyJson` (`@length(min: 1)`), which has no representation for
/// "no document", so the document reported for a resource nothing is attached
/// to is the EMPTY policy document: a JSON object with no statements, which
/// grants exactly what a resource with no policy attached grants — nothing.
/// `PutResourcePolicy` refuses a document with no statements
/// (`::service_common::iam_policy`), so this value is never one a caller
/// stored: reading it back says only that the resource carries no policy.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// How many tags one resource holds.
///
/// `TagList` is `@length(min: 1, max: 200)`, which is the bound on the tags a
/// resource carries as much as on the tags one request names, so a
/// `TagResource` whose merge would pass it is refused rather than storing a set
/// no `ListTagsForResource` could report. A resource's tags are a map, so this
/// bounds the KEYS it carries — the count a merged tag set
/// ([`merge_tags`]) has, and the count of distinct keys the entity side of the
/// same bound takes.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// Merge the tag pairs `incoming` into the tag set `held`.
///
/// A resource's tags are a MAP from key to value — `Tag$Key` is the key "the
/// tag" has, and `ListTagsForResource` reports that map as a `TagList` — so a
/// key occurs at most once in a stored list: a key the set already carries
/// takes the incoming value instead of gaining a second entry, and a key
/// `incoming` itself names more than once keeps the LAST value given for it,
/// since the pairs are read in order and each one is the value of its key from
/// then on. Each merged key is (re)placed at the end, so a set reads in the
/// order its keys were last written.
///
/// Every path that stores tags goes through this, so no stored list can hold
/// one key twice, and the count of a stored list is the count of its KEYS —
/// which is what the per-resource bound [`MAX_RESOURCE_TAGS`] is judged on,
/// here and in
/// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::judge_change_set`].
pub fn merge_tags<I>(held: &mut Vec<Tag>, incoming: I)
where
    I: IntoIterator<Item = Tag>,
{
    for tag in incoming {
        held.retain(|existing| existing.key != tag.key);
        held.push(tag);
    }
}

/// The tag SET a submitted tag list names: [`merge_tags`] into an empty set.
///
/// `TagList` carries no `@uniqueItems`, so a request may name one key twice;
/// what is STORED is the set those pairs describe rather than the list as
/// written.
pub fn tag_set<I>(tags: I) -> Vec<Tag>
where
    I: IntoIterator<Item = Tag>,
{
    let mut set = Vec::new();
    merge_tags(&mut set, tags);
    set
}

/// Whether two stored tag sets are the SAME set: the same keys, each carrying
/// the same value.
///
/// Both sides are [`merge_tags`] sets, so a key occurs at most once in either
/// and equal counts plus one-way containment is set equality. Compared by key
/// and value rather than position: a resource's tags are a MAP, so the order a
/// list reports them in is not part of what the resource carries.
pub fn is_same_tag_set(left: &[Tag], right: &[Tag]) -> bool {
    left.len() == right.len()
        && left.iter().all(|tag| {
            right
                .iter()
                .any(|other| other.key == tag.key && other.value == tag.value)
        })
}

/// A write handle on the tags of ONE stored resource, carrying with them the
/// instant that record reports as its last modification — when its kind
/// publishes one.
///
/// Handed out by
/// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::resource_tags_write`] so
/// that a tag write reaches the whole record rather than its tag list alone:
/// writing a resource's tags MODIFIES that resource, and what the record then
/// reports as the last time it was modified is part of what the write leaves
/// behind.
#[derive(Debug)]
pub struct ResourceTagsWrite<'a> {
    tags: &'a mut Vec<Tag>,
    last_modified_date: Option<&'a mut String>,
}

impl ResourceTagsWrite<'_> {
    /// The tag set the record holds now — what the set a tag write stores is
    /// computed from.
    pub fn held(&self) -> &[Tag] {
        self.tags
    }

    /// Leave the record holding the tag set `tags`, written at `at`.
    ///
    /// A write leaving a DIFFERENT set modifies the resource, so the record's
    /// last-modified instant becomes `at` — the restamp the tag write a change
    /// set carries performs
    /// ([`ComAmazonawsMarketplacecatalogAwsmpseymourState::apply_changes`]),
    /// which is what the entity member documented as the last time the entity
    /// was modified reports and what the entity list filters and orders on. A
    /// write that leaves the set it found modifies nothing, so it moves no
    /// instant. A record whose kind publishes no last-modified instant — a
    /// change set, whose timestamps are those of the run it is — takes the
    /// tags alone.
    pub fn store(&mut self, tags: Vec<Tag>, at: SystemTime) {
        if is_same_tag_set(self.tags, &tags) {
            return;
        }
        *self.tags = tags;
        if let Some(stamp) = self.last_modified_date.as_mut() {
            **stamp = iso8601(at);
        }
    }
}

/// The tag-key namespace Amazon Web Services keeps for the tags it generates
/// itself.
///
/// `Tag$Key`'s `@pattern` admits a colon, so the key SYNTAX cannot state this
/// rule: AWS's tagging rules keep the `aws:` namespace for the tags AWS applies
/// on a caller's behalf, and a caller can neither create, edit nor remove a key
/// inside it. Every request member of this service that carries tag keys is
/// judged against it through [`is_reserved_tag_key`], so a key AWS owns is
/// refused rather than stored as a user tag the tag reports would hand back.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies inside the namespace [`RESERVED_TAG_KEY_PREFIX`] keeps.
///
/// The prefix is matched however it is cased: it names ONE namespace, and
/// `Tag$Key`'s pattern admits every casing of it, so spelling it in capitals
/// does not put a key outside it.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first of `keys`, in the order given, that [`is_reserved_tag_key`] — the
/// key a refusal names — or `None` when every key is one a caller may name.
pub fn first_reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// The same, for the tag pairs a request carries.
pub fn first_reserved_tag(tags: &[Tag]) -> Option<&str> {
    first_reserved_tag_key(tags.iter().map(|tag| tag.key.as_str()))
}

/// The `NextToken` prefix `ListChangeSets` pages with.
pub const CHANGE_SET_PAGE_PREFIX: &str = "CS";

/// The `NextToken` prefix `ListEntities` pages with.
pub const ENTITY_PAGE_PREFIX: &str = "ENT";

/// The largest page `ListAssessments` serves:
/// `ListAssessmentsMaxResultInteger` is `@range(min: 1, max: 100)`. Its
/// `MaxResults` is `@default(20)` ("If a value isn't provided, the default value
/// is 20"), which the boundary applies before a handler runs.
pub const LIST_ASSESSMENTS_PAGE_LIMIT: i32 = 100;

/// The page `ListAssessments` serves when the request names no `MaxResults`:
/// the `@default(20)` on `ListAssessmentsRequest$MaxResults`. The boundary
/// applies it, so the handler is handed this value rather than nothing.
pub const LIST_ASSESSMENTS_PAGE_DEFAULT: i32 = 20;

/// The largest page `DescribeAssessment` serves of one assessment's
/// `ControlAssessments`: `DescribeAssessmentMaxResultInteger` is
/// `@range(min: 1, max: 100)`, `@default(50)`.
pub const DESCRIBE_ASSESSMENT_PAGE_LIMIT: i32 = 100;

/// The `ControlAssessments` page `DescribeAssessment` serves when the request
/// names no `MaxResults`: the `@default(50)` on
/// `DescribeAssessmentRequest$MaxResults`, applied by the boundary.
pub const DESCRIBE_ASSESSMENT_PAGE_DEFAULT: i32 = 50;

/// The `NextToken` prefix `ListAssessments` pages with.
pub const ASSESSMENT_PAGE_PREFIX: &str = "ASMT";

/// The `NextToken` prefix `DescribeAssessment` pages its `ControlAssessments`
/// with.
pub const CONTROL_PAGE_PREFIX: &str = "CTRL";

/// The prefix of every assessment id, from `AssessmentIdentifier`'s own pattern
/// (`^(assessment-[a-zA-Z0-9]+|arn:…/Assessment/assessment-[a-zA-Z0-9]+)$`) and
/// from `DescribeAssessmentRequest$AssessmentIdentifier`'s doc ("for example,
/// assessment-12345").
pub const ASSESSMENT_ID_PREFIX: &str = "assessment-";

/// The resource type an assessment ARN names, from the same two sources
/// ("arn:aws:aws-marketplace:us-east-1::AWSMarketplace/Assessment/assessment-12345").
pub const ASSESSMENT_ARN_RESOURCE_TYPE: &str = "Assessment";

/// The framework an AMI product's delivery option is evaluated against:
/// `ListAssessments` names it "AMI Security", and the model's
/// `AMISecuritySummary` / `AMISecurityFilters` union members give the id.
pub const FRAMEWORK_AMI_SECURITY: &str = "AMISecurity";

/// The framework a container product's delivery option is evaluated against,
/// from the same two sources (`ContainerSecuritySummary` /
/// `ContainerSecurityFilters`, "Container Security").
pub const FRAMEWORK_CONTAINER_SECURITY: &str = "ContainerSecurity";

/// The frameworks this catalog assesses against, named by the `FrameworkSummary`
/// and `FrameworkFilters` union members the model declares (`AMISecuritySummary`
/// / `AMISecurityFilters` -> `AMISecurity`, `ContainerSecuritySummary` /
/// `ContainerSecurityFilters` -> `ContainerSecurity`); `FrameworkId`'s doc gives
/// `AMISecurity` as the unversioned form and `DescribeAssessmentResponse$FrameworkId`'s
/// the versioned one ("frameworkId@version (for example, AMISecurity@1.0)").
pub const ASSESSMENT_FRAMEWORKS: &[&str] =
    &[FRAMEWORK_AMI_SECURITY, FRAMEWORK_CONTAINER_SECURITY];

/// The framework version an assessment of this catalog reports.
///
/// `DescribeAssessmentResponse$FrameworkId` is the versioned id
/// ("frameworkId@version (for example, AMISecurity@1.0)") while
/// `ListAssessmentsRequest$FrameworkId` is the unversioned one ("For example,
/// AMISecurity"), so a report has to name a version: this is the one the
/// service's own documentation spells, and the only one this emulator
/// evaluates.
pub const ASSESSMENT_FRAMEWORK_VERSION: &str = "1.0";

/// The framework the catalog evaluates a product of `entity_type` against, or
/// `None` for an entity type neither framework covers.
///
/// `ListAssessments` documents an assessment as "the result of evaluating a
/// product or change set against a framework, such as AMI Security or Container
/// Security", and the model pairs each framework with the delivery form it
/// assesses — so an AMI product's delivery options are evaluated against
/// [`FRAMEWORK_AMI_SECURITY`], a container product's against
/// [`FRAMEWORK_CONTAINER_SECURITY`], and the catalog's other entity types
/// ([`ENTITY_TYPES`]) against neither, since no framework of this service
/// describes them.
pub fn entity_framework(entity_type: &str) -> Option<&'static str> {
    match entity_type {
        "AmiProduct" => Some(FRAMEWORK_AMI_SECURITY),
        "ContainerProduct" => Some(FRAMEWORK_CONTAINER_SECURITY),
        _ => None,
    }
}

/// The framework one `FrameworkFilters` member filters by, and the delivery
/// option that member narrows to.
///
/// `FrameworkFilters` is "framework-specific filters … Set exactly one member,
/// corresponding to the framework you want to filter by", and each member's own
/// documentation names its framework ("filters that apply to assessments
/// performed against the AMI Security framework"), so the member a request
/// names IS a framework of [`ASSESSMENT_FRAMEWORKS`] — which is what lets a
/// caller's own `FrameworkId` be compared with the framework its filters belong
/// to, as well as what selects records.
pub fn framework_filters_scope(filters: &FrameworkFilters) -> (&'static str, Option<&str>) {
    match filters {
        FrameworkFilters::AMISecurityFilters(filter) => {
            (FRAMEWORK_AMI_SECURITY, filter.delivery_option_id.as_deref())
        }
        FrameworkFilters::ContainerSecurityFilters(filter) => (
            FRAMEWORK_CONTAINER_SECURITY,
            filter.delivery_option_id.as_deref(),
        ),
    }
}

/// How long an assessment stays valid: the span between an assessment's
/// `CreatedAt` and its `ExpiresAt`.
///
/// `AssessmentSummary$ExpiresAt` is "the date and time the assessment expires"
/// and neither the model nor the operation's documentation states how long that
/// is, so this span is the emulator's OWN rather than a claim about AWS's.
/// Nothing of the model is keyed to an expired assessment — there is no status,
/// filter or error for one — so the instant is reported and never acted on.
pub const ASSESSMENT_VALIDITY: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The details member a change submits a product's delivery options in.
///
/// A change's details belong to its change type, which the model declares no
/// shape for, so the member names a change carries come from the catalog's own
/// change documents rather than from a modelled shape: a change that adds or
/// updates a product version carries its delivery options in a list under this
/// member, each entry carrying [`DELIVERY_OPTION_ID_MEMBER`] once the catalog
/// has assigned one and [`DELIVERY_OPTION_DETAILS_MEMBER`] holding the delivery
/// form's own members.
pub const DELIVERY_OPTIONS_DETAIL_MEMBER: &str = "DeliveryOptions";

/// The member of one submitted delivery option that carries its id — what
/// `FrameworkSummary$DeliveryOptionId` names, and what
/// `AMISecurityFilters$DeliveryOptionId` selects assessments by.
pub const DELIVERY_OPTION_ID_MEMBER: &str = "Id";

/// The member of one submitted delivery option that carries the delivery
/// form's own members, which is what a framework's controls read.
pub const DELIVERY_OPTION_DETAILS_MEMBER: &str = "Details";

/// The prefix of a delivery option id this catalog assigns, in the
/// `ResourceId` form (`^[\w\-]+$`) `FrameworkSummary$DeliveryOptionId` targets.
pub const DELIVERY_OPTION_ID_PREFIX: &str = "do-";

/// How many delivery options one change may submit.
///
/// The model declares no shape for a change's details, so it states no bound on
/// the list under [`DELIVERY_OPTIONS_DETAIL_MEMBER`] — `Change$Details` is
/// `@length(max: 16384)` but `Change$DetailsDocument` carries the same value as
/// a document with no length at all — and every option in that list is one the
/// catalog assigns an id to and records an assessment for, so an unbounded list
/// is an unbounded amount of state one request leaves behind. This is therefore
/// the emulator's OWN bound on that list rather than a claim about AWS's, chosen
/// as `RequestedChangeList`'s own modelled maximum so one change set is bounded
/// the same way whether it carries its work as changes or as delivery options. A
/// change past it is refused ([`too_many_delivery_options`]) rather than stored.
pub const MAX_DELIVERY_OPTIONS_PER_CHANGE: usize = 20;

/// How many changes one change set carries: `RequestedChangeList` is
/// `@length(min: 1, max: 20)`.
///
/// The modelled bound, re-stated here because `StartChangeSet` re-reads it
/// before it allocates or scans from the submitted count: that count is what
/// sizes the scan for a repeated change pair and the records the call leaves in
/// the store, so the work and the state one request leaves behind stay bounded
/// by this constant and not by the list the request sent. A request past it is
/// refused ([`too_many_changes`]).
pub const MAX_CHANGES_PER_CHANGE_SET: usize = 20;

/// How long an assessment a caller STARTED runs before its result and its
/// control results are reported.
///
/// `StartAssessment` documents the work as asynchronous ("the assessment runs
/// asynchronously. This action returns the ID of the assessment immediately. To
/// get its status and the results of each control, use the DescribeAssessment
/// action") and neither the model nor the documentation says how long one
/// takes, so this span is the emulator's OWN rather than a claim about AWS's. It
/// is non-zero so that a caller reading straight after the start observes the
/// run it started — and so that the entity being assessed is in use for as long
/// as that run lasts ([`assessment_in_progress`]). The due instant is stored on
/// the record ([`AssessmentRecord::runs_until`]) and read by whichever later
/// call reports it; nothing sleeps for it.
pub const ASSESSMENT_RUN: Duration = Duration::from_secs(3);

/// How many assessments of this account may be RUNNING at once.
///
/// `StartAssessment` models a `ServiceQuotaExceededException` for "the maximum
/// number of open requests per account has been exceeded" and publishes no
/// number for it, so the bound itself is the emulator's OWN: an open request is
/// an assessment still running ([`AssessmentRecord::is_running`]), far more at
/// once than a workflow an emulator serves starts, and what keeps the work one
/// caller can have in flight bounded.
pub const OPEN_ASSESSMENT_LIMIT: usize = 10;

/// How many assessments the store holds.
///
/// On the same footing as [`CHANGE_SET_QUOTA`]: the emulator's own bound on the
/// state the assessment surface accumulates, far above any workflow an emulator
/// serves, and what stops it growing without end across change sets. At the
/// bound the OLDEST assessment is dropped to make room, which is the one a
/// report ordered newest first reaches last.
pub const ASSESSMENT_QUOTA: usize = 1_000;

/// A `ControlAssessmentResult` value: the control's condition held.
pub const CONTROL_PASS: &str = "PASS";

/// A `ControlAssessmentResult` value: the control's condition did not hold, and
/// the control reports the `ControlError` that says why.
pub const CONTROL_FAIL: &str = "FAIL";

/// A `ControlAssessmentResult` value: the assessed delivery option carries
/// nothing this control reads, so nothing was evaluated — which is reported as
/// such rather than as a pass the catalog never established.
pub const CONTROL_NOT_EXECUTED: &str = "NOT_EXECUTED";

/// An `AssessmentResult` value: no control of the framework failed.
pub const ASSESSMENT_PASS: &str = "PASS";

/// An `AssessmentResult` value: at least one control of the framework failed.
pub const ASSESSMENT_FAIL: &str = "FAIL";

/// A `ControlError$Code` (`^[A-Z_]+$`): the delivery option does not carry the
/// member this control reads.
pub const CONTROL_ERROR_MISSING_VALUE: &str = "MISSING_VALUE";

/// A `ControlError$Code`: the delivery option carries the member, with a value
/// the control refuses.
pub const CONTROL_ERROR_INVALID_VALUE: &str = "INVALID_VALUE";

/// How much of a submitted value a `ControlError$Scope` entry carries:
/// `ScopeValue` is `@length(1..1024)`.
pub const SCOPE_VALUE_LIMIT: usize = 1024;

/// A `ChangeStatus` value.
pub const STATUS_PREPARING: &str = "PREPARING";
/// A `ChangeStatus` value.
pub const STATUS_APPLYING: &str = "APPLYING";
/// A `ChangeStatus` value.
pub const STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// A `ChangeStatus` value.
pub const STATUS_CANCELLED: &str = "CANCELLED";
/// A `ChangeStatus` value: the terminal status of a change set whose changes
/// could not be applied.
pub const STATUS_FAILED: &str = "FAILED";

/// The `FailureCode` of a change set that failed on what its request asked
/// for: `FailureCode` documents "CLIENT_ERROR, which means that there are
/// issues with the request (see the ErrorDetailList)".
pub const FAILURE_CODE_CLIENT_ERROR: &str = "CLIENT_ERROR";

/// The other `FailureCode`: "SERVER_FAULT, which means that there is a problem
/// in the system, and you should retry your request". A change set of this
/// emulator fails only on its own content, so nothing reports this value — the
/// emulator does not fabricate a system problem.
pub const FAILURE_CODE_SERVER_FAULT: &str = "SERVER_FAULT";

/// The `ErrorDetail$ErrorCode` of a change whose details give a member the
/// catalog reads a value that member cannot carry.
///
/// `ErrorCodeString` is `@length(1..72)`, `^[a-zA-Z_]+$`, which every code
/// here stays inside.
pub const ERROR_CODE_INVALID_VALUE: &str = "INVALID_VALUE";

/// The `ErrorDetail$ErrorCode` of a change whose `EntityTags` would leave its
/// entity holding more than [`MAX_RESOURCE_TAGS`] tags.
pub const ERROR_CODE_LIMIT_EXCEEDED: &str = "LIMIT_EXCEEDED";

/// How much of a submitted details value an `ErrorDetail$ErrorMessage` or a
/// `FailureDescription` quotes, so a message built from a large value stays
/// inside `ExceptionMessageContent`'s 2048 characters.
pub const DETAIL_VALUE_QUOTE_LIMIT: usize = 256;

/// How long a message this service builds around a submitted value may be: the
/// 2048 characters `ExceptionMessageContent` and `BatchDescribeErrorMessageContent`
/// both declare, less the two brackets [`validation_message`] wraps a top-level
/// fault's sentence in. [`quoting`] cuts a quote to whatever room the sentence
/// around it leaves inside this. A sentence that is NOT wrapped in those
/// brackets — a `BatchDescribeErrorDetail$ErrorMessage`, a missing-resource
/// message — is held to the same budget, two characters short of its own bound,
/// rather than to a second constant.
const MESSAGE_BUDGET: usize = 2046;

/// What a cut quote ends with, so a reader sees the value was cut rather than
/// sent that way.
const CUT_MARK: &str = "...";

/// An `Intent` value — `StartChangeSetRequest$Intent` documents "The default is
/// APPLY".
pub const INTENT_APPLY: &str = "APPLY";
/// An `Intent` value: "To test your request before applying changes to your
/// entities, use VALIDATE".
pub const INTENT_VALIDATE: &str = "VALIDATE";

/// A status change the clock owes a change set: the instant it falls due, and
/// the status the change set carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One `Change` of a change set, as submitted and then as resolved.
#[derive(Debug, Clone)]
pub struct ChangeRecord {
    pub change_type: String,
    pub entity_type: String,
    /// `Entity$Identifier` exactly as submitted, which is absent for a change
    /// that creates an entity.
    pub entity_identifier: Option<String>,
    /// `Change$Details` as submitted.
    pub details: Option<String>,
    /// `Change$DetailsDocument` as submitted.
    pub details_document: Option<Value>,
    pub change_name: Option<String>,
    pub entity_tags: Vec<Tag>,
    /// The entity this change acts on: the id `Entity$Identifier` resolved to,
    /// or the id reserved for the entity the change creates.
    ///
    /// Resolved from the REQUEST alone, so a change resolves to an entity
    /// whatever `Intent` the change set carries: what the intent decides is
    /// whether the entities are WRITTEN when the run ends, not which entity
    /// each change is judged against.
    pub entity_id: String,
    /// Whether this change creates the entity [`Self::entity_id`] names — it
    /// named no existing one — which is the change a `VALIDATE` change set
    /// judges but never applies.
    pub creates: bool,
    /// `ChangeSummary$ErrorDetailList`: what applying this change found wrong
    /// with it. Empty until the change set's changes are applied, and empty
    /// afterwards for a change that was applicable.
    pub errors: Vec<ErrorDetail>,
}

impl ChangeRecord {
    /// The details this change carries, as one JSON value — whichever of
    /// `Details` and `DetailsDocument` it was submitted with.
    ///
    /// `Change$Details` is a JSON string and `Change$DetailsDocument` the same
    /// value as a document ("You can use either `Details` or
    /// `DetailsDocument`, but not both"), so both land here as one value.
    pub fn details_value(&self) -> Option<Value> {
        if let Some(text) = self.details.as_deref() {
            return serde_json::from_str(text).ok();
        }
        self.details_document.clone()
    }

    /// The `ChangeSummary` projection `DescribeChangeSet` reports.
    pub fn to_summary(&self) -> ChangeSummary {
        ChangeSummary {
            change_type: Some(self.change_type.clone()),
            entity: Some(Entity {
                r#type: self.entity_type.clone(),
                identifier: self.entity_identifier.clone(),
            }),
            details: self.details.clone(),
            details_document: self
                .details_document
                .as_ref()
                // `None` is unreachable: a document deeper than
                // `MAX_DETAILS_DEPTH` is refused before it is stored.
                .and_then(json_to_document),
            // "An array of ErrorDetail objects associated with the change":
            // reported by a change applying found something wrong with, and
            // absent for every other change.
            error_detail_list: (!self.errors.is_empty()).then(|| self.errors.clone()),
            change_name: self.change_name.clone(),
        }
    }
}

/// One change set, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ChangeSetRecord {
    /// `ResourceId`'s `^[\w\-]+$`, minted by
    /// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::mint_change_set_id`].
    pub id: String,
    pub arn: String,
    /// The change set's `ChangeSetName`: the one the start named, or the one
    /// the service set for a start that named none ([`default_change_set_name`]).
    /// Never absent, so every change set reports a name and can be selected by
    /// one.
    pub name: String,
    /// One of `Intent`'s values.
    pub intent: String,
    /// One of `ChangeStatus`'s values.
    pub status: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub start_time: String,
    pub end_time: Option<String>,
    pub failure_code: Option<String>,
    pub failure_description: Option<String>,
    pub changes: Vec<ChangeRecord>,
    pub tags: Vec<Tag>,
    /// The region this change set was started in — what the ARNs of the
    /// entities it applies are built for.
    pub region: String,
    /// The status changes the clock still owes this change set, oldest first.
    pub schedule: Vec<PendingTransition>,
}

impl ChangeSetRecord {
    /// True while the change set is still running, so the entities its changes
    /// target are locked: `StartChangeSet` holds the lock "until the change set
    /// has completed (either succeeded, cancelled, or failed)", which is every
    /// status before a terminal one.
    ///
    /// Running is NOT the same as cancellable — see [`Self::is_cancellable`].
    pub fn is_running(&self) -> bool {
        self.status == STATUS_PREPARING || self.status == STATUS_APPLYING
    }

    /// True while the change set is still an OPEN request, which is the only
    /// window `CancelChangeSet` serves: its documentation requires the cancel
    /// to be "sent before the status of the request changes to `APPLYING`, the
    /// final stage of completing your change request", so a change set that has
    /// entered that final stage is past cancelling, as is one that has reached a
    /// terminal status.
    pub fn is_cancellable(&self) -> bool {
        self.status == STATUS_PREPARING
    }

    /// The entities this change set acts on, in submission order and without
    /// repeats — `ChangeSetSummaryListItem$EntityIdList`.
    pub fn entity_ids(&self) -> Vec<String> {
        let validating = self.intent == INTENT_VALIDATE;
        let mut ids: Vec<String> = Vec::with_capacity(self.changes.len());
        for change in &self.changes {
            // A validation applies nothing, so the entities it is a part of are
            // the ones its changes NAMED: the id a creating change resolved to
            // is reserved for judging the request, and no entity carries it.
            if validating && change.creates {
                continue;
            }
            if !ids.iter().any(|seen| seen == &change.entity_id) {
                ids.push(change.entity_id.clone());
            }
        }
        ids
    }

    /// The `ChangeSetSummaryListItem` projection `ListChangeSets` reports.
    pub fn to_summary(&self) -> ChangeSetSummaryListItem {
        ChangeSetSummaryListItem {
            change_set_id: Some(self.id.clone()),
            change_set_arn: Some(self.arn.clone()),
            change_set_name: Some(self.name.clone()),
            start_time: Some(self.start_time.clone()),
            end_time: self.end_time.clone(),
            status: Some(self.status.clone()),
            // `EntityIdList` is documented "a maximum of 20 entities. It must
            // contain at least one entity", so a change set holding no entity
            // id — a `VALIDATE` change set whose changes all create, since a
            // validation applies nothing — reports NO member rather than an
            // empty list that bound excludes. The same decision `Tags` takes
            // for its own `@length(min: 1 …)` list.
            entity_id_list: {
                let ids = self.entity_ids();
                (!ids.is_empty()).then_some(ids)
            },
            failure_code: self.failure_code.clone(),
        }
    }

    /// Whether this change set satisfies the `ListChangeSets` filter `name`
    /// with `values`, as `Filter$ValueList`'s documentation defines each one.
    ///
    /// A name outside [`CHANGE_SET_FILTER_NAMES`] never reaches here, and
    /// neither does a value outside the set a filter's attribute publishes
    /// ([`filter_value_set`]) or outside the form that attribute's values take
    /// ([`filter_value_form`]): the shared filter rule refuses the request
    /// first, so what reaches here is only ever compared with the state.
    pub fn matches_filter(&self, name: &str, values: &[String]) -> bool {
        let bound = values.first().map(String::as_str).unwrap_or_default();
        match name {
            "ChangeSetName" => values.iter().any(|value| value == &self.name),
            "Status" => values.iter().any(|value| value == &self.status),
            "EntityId" => self
                .entity_ids()
                .iter()
                .any(|id| values.iter().any(|value| value == id)),
            // The four time filters compare the `DateTimeISO8601` strings the
            // record holds, which are fixed-width and UTC, so their byte order
            // is their chronological order.
            "BeforeStartTime" => self.start_time.as_str() < bound,
            "AfterStartTime" => self.start_time.as_str() > bound,
            "BeforeEndTime" => self
                .end_time
                .as_deref()
                .is_some_and(|held| held < bound),
            "AfterEndTime" => self.end_time.as_deref().is_some_and(|held| held > bound),
            _ => false,
        }
    }

    /// The value this change set sorts on under `Sort$SortBy` value `sort_by`.
    ///
    /// A change set that is still running has no `EndTime`, and the empty
    /// string it sorts as puts it first ascending and last descending.
    pub fn sort_key(&self, sort_by: &str) -> &str {
        match sort_by {
            "EndTime" => self.end_time.as_deref().unwrap_or_default(),
            _ => self.start_time.as_str(),
        }
    }
}

/// One catalog entity, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct EntityRecord {
    /// `ResourceId`'s `^[\w\-]+$`.
    pub id: String,
    /// `EntityType`'s `^[a-zA-Z]+$`.
    pub entity_type: String,
    /// The SCHEMA VERSION the entity's type was last addressed at: the
    /// `@<version>` part of the `Change$Entity$Type` of the most recent change
    /// applied to this record ([`entity_type_version_of`]).
    ///
    /// Kept beside the type because the two entity READS report the type at
    /// that version ([`EntityRecord::entity_type_reference`]), while the type
    /// alone is what the record is filtered, listed and named by.
    pub entity_type_version: Option<String>,
    pub arn: String,
    /// How many change sets have applied a change to this entity. It is what
    /// makes `EntityIdentifier` (`<id>@<revision>`) move when the entity does.
    pub revision: u32,
    pub name: Option<String>,
    /// One of the values this entity's own type publishes
    /// ([`entity_type_visibility_values`]).
    pub visibility: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub last_modified_date: String,
    /// The details the applied changes have accumulated, as one JSON object.
    pub details: Value,
    pub tags: Vec<Tag>,
}

impl EntityRecord {
    /// The entity's type at its schema version (`EntityType@Version`), which is
    /// the form the two members documented as carrying it report —
    /// `DescribeEntityResponse$EntityType` and `EntityDetail$EntityType`, each
    /// documented "in the format of `EntityType@Version`"
    /// (`smithy_query(query="shape", shape="DescribeEntityResponse,EntityDetail")`).
    ///
    /// The version is the one the changes that wrote this record addressed its
    /// type at. A record written without one — which no accepted change leaves,
    /// since `Change$Entity$Type` must name a version
    /// ([`check_entity_type_reference`]) — reports the type alone, the most of
    /// the documented form that is known.
    pub fn entity_type_reference(&self) -> String {
        match &self.entity_type_version {
            Some(version) => {
                format!("{}{ENTITY_TYPE_VERSION_SEPARATOR}{version}", self.entity_type)
            }
            None => self.entity_type.clone(),
        }
    }

    /// `EntityIdentifier`: the entity id and the revision the reader is seeing,
    /// which is the form `Entity$Identifier` accepts back (`^[\w\-@]+$`).
    pub fn identifier(&self) -> String {
        format!("{}@{}", self.id, self.revision)
    }

    /// The accumulated details as the `Json` string `Details` is modelled as.
    ///
    /// Inside `Json`'s `@length(max)` ([`DETAILS_JSON_LIMIT`]) for every record
    /// this store holds: a change set is applied only once the merge its
    /// changes describe has been judged against that bound
    /// ([`ComAmazonawsMarketplacecatalogAwsmpseymourState::judge_change_set`]),
    /// so nothing longer is ever written here to report.
    pub fn details_json(&self) -> String {
        self.details.to_string()
    }

    /// `{ProductTitle, Visibility}` for the one per-type summary member
    /// `entity_type` names, and `None` for every other entity's.
    ///
    /// The [`PRODUCT_ENTITY_TYPES`] test is what keeps the member and the type
    /// it belongs to in step: only a type in that catalog has a summary of this
    /// shape.
    fn product_summary(&self, entity_type: &str) -> Option<(Option<String>, Option<String>)> {
        (self.entity_type == entity_type && PRODUCT_ENTITY_TYPES.contains(&entity_type))
            .then(|| (self.summary_name(), self.summary_visibility()))
    }

    /// The `EntitySummary` projection `ListEntities` reports.
    pub fn to_summary(&self) -> EntitySummary {
        EntitySummary {
            name: self.summary_name(),
            entity_type: Some(self.entity_type.clone()),
            entity_id: Some(self.id.clone()),
            entity_arn: Some(self.arn.clone()),
            last_modified_date: Some(self.last_modified_date.clone()),
            visibility: Some(self.visibility.clone()),
            ami_product_summary: self.product_summary("AmiProduct").map(
                |(product_title, visibility)| AmiProductSummary {
                    product_title,
                    visibility,
                },
            ),
            container_product_summary: self.product_summary("ContainerProduct").map(
                |(product_title, visibility)| ContainerProductSummary {
                    product_title,
                    visibility,
                },
            ),
            data_product_summary: self.product_summary("DataProduct").map(
                |(product_title, visibility)| DataProductSummary {
                    product_title,
                    visibility,
                },
            ),
            saa_s_product_summary: self.product_summary("SaaSProduct").map(
                |(product_title, visibility)| SaaSProductSummary {
                    product_title,
                    visibility,
                },
            ),
            machine_learning_product_summary: self.product_summary("MachineLearningProduct").map(
                |(product_title, visibility)| MachineLearningProductSummary {
                    product_title,
                    visibility,
                },
            ),
            // The five remaining summaries name members of their own, each the
            // name of the details member the entity carries that value in.
            offer_summary: self.is_type("Offer").then(|| OfferSummary {
                name: self.summary_name(),
                product_id: self.summary_text("ProductId"),
                resale_authorization_id: self.summary_text("ResaleAuthorizationId"),
                subsidiary_authorization_id: self.summary_text("SubsidiaryAuthorizationId"),
                release_date: self.summary_text("ReleaseDate"),
                availability_end_date: self.summary_text("AvailabilityEndDate"),
                buyer_accounts: self.summary_texts("BuyerAccounts"),
                state: self.summary_text("State"),
                targeting: self.summary_texts("Targeting"),
                offer_set_id: self.summary_text("OfferSetId"),
                target_agreement_id: self.summary_text("TargetAgreementId"),
                target_agreement_intent: self.summary_text("TargetAgreementIntent"),
                created_by_source: self.summary_text("CreatedBySource"),
            }),
            resale_authorization_summary: self.is_type("ResaleAuthorization").then(|| {
                ResaleAuthorizationSummary {
                    name: self.summary_name(),
                    product_id: self.summary_text("ProductId"),
                    product_name: self.summary_text("ProductName"),
                    manufacturer_account_id: self.summary_text("ManufacturerAccountId"),
                    manufacturer_legal_name: self.summary_text("ManufacturerLegalName"),
                    reseller_account_id: self.summary_text("ResellerAccountID"),
                    reseller_legal_name: self.summary_text("ResellerLegalName"),
                    status: self.summary_text("Status"),
                    offer_extended_status: self.summary_text("OfferExtendedStatus"),
                    created_date: self.summary_text("CreatedDate"),
                    availability_end_date: self.summary_text("AvailabilityEndDate"),
                    reseller_role: self.summary_text("ResellerRole"),
                    source_authorization: self.summary_text("SourceAuthorization"),
                    issuer_account_id: self.summary_text("IssuerAccountId"),
                }
            }),
            offer_set_summary: self.is_type("OfferSet").then(|| OfferSetSummary {
                name: self.summary_name(),
                state: self.summary_text("State"),
                release_date: self.summary_text("ReleaseDate"),
                associated_offer_ids: self.summary_texts("AssociatedOfferIds"),
                solution_id: self.summary_text("SolutionId"),
            }),
            express_private_offer_configuration_summary: self
                .is_type("ExpressPrivateOfferConfiguration")
                .then(|| ExpressPrivateOfferConfigurationSummary {
                    product_id: self.summary_text("ProductId"),
                    state: self.summary_text("State"),
                }),
            subsidiary_authorization_summary: self.is_type("SubsidiaryAuthorization").then(|| {
                SubsidiaryAuthorizationSummary {
                    name: self.summary_name(),
                    status: self.summary_text("Status"),
                    resource_id: self.summary_text("ResourceId"),
                    resource_name: self.summary_text("ResourceName"),
                    subsidiary_account_id: self.summary_text("SubsidiaryAccountId"),
                    created_date: self.summary_text("CreatedDate"),
                }
            }),
        }
    }

    /// Whether this entity is of `entity_type`.
    fn is_type(&self, entity_type: &str) -> bool {
        self.entity_type == entity_type
    }

    /// The `Name` this entity's summary reports: the name it holds, when that
    /// name satisfies the contract the model declares for the members that
    /// report it ([`entity_name_contract`]).
    ///
    /// Judged here as well as on the write path, exactly as an entity's
    /// `Visibility` is ([`details_defects`] fails the change set and
    /// [`detail_visibility`] drops the value): a value a summary member cannot
    /// hold is reported by no projection of this record.
    fn summary_name(&self) -> Option<String> {
        let name = self.name.clone()?;
        entity_name_contract(&self.entity_type)
            .admits_text(&name)
            .then_some(name)
    }

    /// The `Visibility` this entity's per-type summary reports: the value the
    /// record holds, when the enum that summary member targets publishes it
    /// ([`entity_visibility_contract`]).
    ///
    /// Judged here as well as on the write path, for the same reason
    /// [`EntityRecord::summary_name`] is.
    fn summary_visibility(&self) -> Option<String> {
        entity_visibility_contract(&self.entity_type)
            .admits_text(&self.visibility)
            .then(|| self.visibility.clone())
    }

    /// The string this entity's summary reports at `member`: the string the
    /// details carry there, when it satisfies the contract the model declares
    /// for the summary member of that name ([`summary_member_contract`]).
    ///
    /// Judged twice for the same reason as [`EntityRecord::summary_name`]. A
    /// member no per-type summary reports is bounded by nothing, so what the
    /// details hold is what is read.
    fn summary_text(&self, member: &str) -> Option<String> {
        let held = self.details.get(member)?;
        if summary_member_contract(&self.entity_type, member)
            .is_some_and(|contract| !contract.admits(held))
        {
            return None;
        }
        self.detail_text(member)
    }

    /// The list of strings this entity's summary reports at `member`, under the
    /// same rule as [`EntityRecord::summary_text`]: the list the details carry
    /// there, when the list and every value in it satisfy the contract declared
    /// for the summary member of that name.
    fn summary_texts(&self, member: &str) -> Option<Vec<String>> {
        let held = self.details.get(member)?;
        if summary_member_contract(&self.entity_type, member)
            .is_some_and(|contract| !contract.admits(held))
        {
            return None;
        }
        self.detail_texts(member)
    }

    /// The string the entity's details carry at `member`, if any.
    pub fn detail_text(&self, member: &str) -> Option<String> {
        self.details.get(member)?.as_str().map(str::to_string)
    }

    /// The strings the entity's details carry at `member` as a list, if any.
    pub fn detail_texts(&self, member: &str) -> Option<Vec<String>> {
        let items = self.details.get(member)?.as_array()?;
        Some(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
        )
    }

    /// Whether this entity satisfies the `ListEntities` filter `name` with
    /// `values`: `Filter$Name` documents `EntityId` as the one it supports, and
    /// `Filter$ValueList` as "a list of unique `EntityId`s".
    pub fn matches_filter(&self, name: &str, values: &[String]) -> bool {
        match name {
            "EntityId" => values.iter().any(|value| value == &self.id),
            _ => false,
        }
    }

    /// What this entity carries at the attribute `member` names, as the values a
    /// filter matches and a sort orders on.
    ///
    /// Every attribute a request can select or order on — the two `Sort$SortBy`
    /// publishes for every type, and the fields each type's own filter and sort
    /// shapes add — is named after the `EntitySummary` member that reports it, so
    /// one lookup serves all of them: the three common members the record holds
    /// in their own fields, and every other member the details member of that
    /// name. A list-valued attribute (an offer's buyer accounts) answers every
    /// value it holds; an attribute the entity does not carry answers none.
    ///
    /// The one attribute held BOTH in a field and in the details is the entity's
    /// name, under the member its own type names it ([`entity_name_detail_member`]):
    /// that attribute answers the field when the details carry nothing there, so
    /// the value a request selects and orders on is the value the summaries
    /// report. The member ANOTHER type names a name under is no attribute of
    /// this entity, so it answers only what the details hold at it.
    pub fn attribute_values(&self, member: &str) -> Vec<String> {
        match member {
            "EntityId" => return vec![self.id.clone()],
            "LastModifiedDate" => return vec![self.last_modified_date.clone()],
            ENTITY_VISIBILITY_DETAIL_MEMBER => return vec![self.visibility.clone()],
            _ => {}
        }
        let held = match self.details.get(member) {
            Some(Value::String(text)) => vec![text.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        if held.is_empty() && member == entity_name_detail_member(&self.entity_type) {
            return self.name.clone().into_iter().collect();
        }
        held
    }

    /// The value this entity sorts on under the attribute `sort_by` names.
    ///
    /// An attribute holding several values sorts by its least one, and one the
    /// entity does not carry sorts as the empty string — which puts it first
    /// ascending and last descending, as a change set with no `EndTime` sorts.
    pub fn attribute_sort_key(&self, sort_by: &str) -> String {
        let mut values = self.attribute_values(sort_by);
        values.sort();
        values.into_iter().next().unwrap_or_default()
    }
}

/// One condition a type-specific filter shape puts on one entity attribute.
///
/// Every member of every `*Filters` shape in the model is a shape carrying some
/// of `ValueList` (match any of these values), `WildCardValue` ("matches the
/// provided value as a substring in the actual value") and `DateRange` ("dates
/// between which" the attribute falls) — so one condition carries all three, and
/// each handler member fills the ones its own filter shape declares.
#[derive(Debug, Clone, Default)]
pub struct EntityCriterion {
    /// The attribute, named as [`EntityRecord::attribute_values`] reads it.
    pub member: String,
    pub value_list: Vec<String>,
    pub wild_card_value: Option<String>,
    pub after_value: Option<String>,
    pub before_value: Option<String>,
}

impl EntityCriterion {
    /// A condition on `member` that constrains nothing yet.
    pub fn on(member: &str) -> Self {
        Self {
            member: member.to_string(),
            ..Self::default()
        }
    }

    /// The `ValueList` of the filter shape, if it declares one and carries it.
    pub fn values(mut self, value_list: Option<&Vec<String>>) -> Self {
        self.value_list = value_list.cloned().unwrap_or_default();
        self
    }

    /// The `WildCardValue` of the filter shape, if it declares one and carries
    /// it.
    pub fn wild_card(mut self, value: Option<&String>) -> Self {
        self.wild_card_value = value.cloned();
        self
    }

    /// The `DateRange` bounds of the filter shape, if it declares one and
    /// carries it.
    pub fn range(mut self, after: Option<&String>, before: Option<&String>) -> Self {
        self.after_value = after.cloned();
        self.before_value = before.cloned();
        self
    }

    /// Whether this condition constrains nothing, because the filter shape it
    /// was read from carried no value at all.
    pub fn is_empty(&self) -> bool {
        self.value_list.is_empty()
            && self.wild_card_value.is_none()
            && self.after_value.is_none()
            && self.before_value.is_none()
    }

    /// Whether this condition is the one wildcard a request may carry.
    pub fn is_wild_card(&self) -> bool {
        self.wild_card_value.is_some()
    }

    /// Whether `record` satisfies every part of this condition.
    ///
    /// The parts conjoin: a filter shape carrying both a value list and a
    /// wildcard selects an entity satisfying both. A date range is satisfied by
    /// ONE value inside both of its bounds — the attributes it bounds are
    /// `DateTimeISO8601`, which is fixed-width and UTC, so byte order is
    /// chronological order.
    pub fn matches(&self, record: &EntityRecord) -> bool {
        let held = record.attribute_values(&self.member);
        if !self.value_list.is_empty()
            && !held
                .iter()
                .any(|value| self.value_list.iter().any(|wanted| wanted == value))
        {
            return false;
        }
        if let Some(wild_card) = self.wild_card_value.as_deref() {
            if !held.iter().any(|value| value.contains(wild_card)) {
                return false;
            }
        }
        if self.after_value.is_some() || self.before_value.is_some() {
            let within = held.iter().any(|value| {
                self.after_value
                    .as_deref()
                    .is_none_or(|after| value.as_str() >= after)
                    && self
                        .before_value
                        .as_deref()
                        .is_none_or(|before| value.as_str() <= before)
            });
            if !within {
                return false;
            }
        }
        true
    }
}

/// The sort attributes the model declares for `entity_type`: the values of that
/// type's own `*SortBy` enum.
///
/// `Sort$SortBy` documents "In addition to `LastModifiedDate` and `EntityId`,
/// each `EntityType` might support additional fields", and the model names the
/// whole set per type in that type's own `*SortBy` enum — the enums the
/// `EntityTypeSort` union carries (`AmiProductSortBy`, `ContainerProductSortBy`,
/// `DataProductSortBy`, `ExpressPrivateOfferConfigurationSortBy`,
/// `MachineLearningProductSortBy`, `OfferSortBy`, `OfferSetSortBy`,
/// `ResaleAuthorizationSortBy`, `SaaSProductSortBy`), each of which is rendered
/// from the model into the `*_SORT_BY_VALUES` constant of that name in
/// `crate::ops::types`. This answers those constants themselves, so a value
/// added to or dropped from one of those enums moves the set this service sorts
/// that type on with it. A type the model declares no sort shape for (one
/// outside [`SHAPED_ENTITY_TYPES`]) is ordered by the attributes every type
/// carries ([`ENTITY_SORT_BY`]) alone.
pub fn entity_type_sort_by_values(entity_type: &str) -> &'static [&'static str] {
    match entity_type {
        "AmiProduct" => AMI_PRODUCT_SORT_BY_VALUES,
        "ContainerProduct" => CONTAINER_PRODUCT_SORT_BY_VALUES,
        "DataProduct" => DATA_PRODUCT_SORT_BY_VALUES,
        "ExpressPrivateOfferConfiguration" => EXPRESS_PRIVATE_OFFER_CONFIGURATION_SORT_BY_VALUES,
        "MachineLearningProduct" => MACHINE_LEARNING_PRODUCT_SORT_BY_VALUES,
        "Offer" => OFFER_SORT_BY_VALUES,
        "OfferSet" => OFFER_SET_SORT_BY_VALUES,
        "ResaleAuthorization" => RESALE_AUTHORIZATION_SORT_BY_VALUES,
        "SaaSProduct" => SAA_S_PRODUCT_SORT_BY_VALUES,
        "SubsidiaryAuthorization" => SUBSIDIARY_AUTHORIZATION_SORT_BY_VALUES,
        _ => ENTITY_SORT_BY,
    }
}

/// The `Visibility` values the model declares for `entity_type`: the values of
/// that type's own `*VisibilityString` enum.
///
/// One value is reported by two members, exactly as an entity's name is
/// ([`entity_name_contract`]): `EntitySummary$Visibility` reports it for every
/// type and is bounded only by `VisibilityValue`'s length and pattern, while the
/// summary of a type that declares its own visibility enum reports the same
/// value through that enum (`SaaSProductSummary$Visibility` targets
/// `SaaSProductVisibilityString`) and that type's `*VisibilityFilter` selects on
/// it through the same enum — so for such a type the value has to be one that
/// enum publishes, the narrower of the two. Those enums are rendered from the
/// model into the `*_VISIBILITY_STRING_VALUES` constant of that name in
/// `crate::ops::types`, and this answers those constants themselves, so a value
/// added to or dropped from one of them moves the set this service accepts,
/// stores and reports for that type with it — the five do not publish the same
/// values. A type the model declares no visibility enum for reports its
/// visibility through the common member alone, so what bounds it is the set
/// those enums publish between them ([`ENTITY_VISIBILITY_VALUES`]).
pub fn entity_type_visibility_values(entity_type: &str) -> &'static [&'static str] {
    match entity_type {
        "AmiProduct" => AMI_PRODUCT_VISIBILITY_STRING_VALUES,
        "ContainerProduct" => CONTAINER_PRODUCT_VISIBILITY_STRING_VALUES,
        "DataProduct" => DATA_PRODUCT_VISIBILITY_STRING_VALUES,
        "MachineLearningProduct" => MACHINE_LEARNING_PRODUCT_VISIBILITY_STRING_VALUES,
        "SaaSProduct" => SAA_S_PRODUCT_VISIBILITY_STRING_VALUES,
        _ => ENTITY_VISIBILITY_VALUES,
    }
}

/// The contract the value an entity's `Visibility` is read from
/// ([`ENTITY_VISIBILITY_DETAIL_MEMBER`]) has to satisfy for an entity of
/// `entity_type`: the enumerated contract of the values that type publishes
/// ([`entity_type_visibility_values`]).
pub fn entity_visibility_contract(entity_type: &str) -> DetailContract {
    choice(entity_type_visibility_values(entity_type))
}

/// The attributes a list of `entity_type` sorts on BEYOND [`ENTITY_SORT_BY`]:
/// the attributes the model declares for that type
/// ([`entity_type_sort_by_values`]) with the two every type carries dropped, so
/// a type whose sort shape names nothing else adds none.
pub fn entity_type_sort_fields(entity_type: &str) -> Vec<&'static str> {
    entity_type_sort_by_values(entity_type)
        .iter()
        .copied()
        .filter(|field| !ENTITY_SORT_BY.contains(field))
        .collect()
}

/// Every attribute a `ListEntities` of `entity_type` sorts on: the two
/// [`ENTITY_SORT_BY`] publishes for every type, then that type's own fields.
pub fn entity_sort_attributes(entity_type: &str) -> Vec<&'static str> {
    let mut attributes = ENTITY_SORT_BY.to_vec();
    attributes.extend(entity_type_sort_fields(entity_type));
    attributes
}

/// One assessment, as this emulator persists it: the record
/// `DescribeAssessment` reports and `ListAssessments` summarises.
///
/// An assessment is the RESULT of evaluating an entity or a change set against a
/// framework (`ListAssessments`: "An assessment is the result of evaluating a
/// product or change set against a framework, such as AMI Security or Container
/// Security"), and what submits a product to such an evaluation is a change set:
/// a succeeded `Intent=APPLY` change set that adds or updates a delivery option
/// of a product one of this catalog's frameworks covers ([`entity_framework`])
/// is what creates a record here, one per delivery option it submitted, as
/// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::apply_changes`] applies
/// that change.
///
/// Every value below is COMPUTED from what the change submitted: the controls
/// are the framework's own checks run over the delivery option's details
/// ([`assess_delivery_option`]), and the overall result is `FAIL` exactly when
/// one of them failed. Nothing here reports a judgement the catalog did not
/// make — a control the delivery option carries nothing for is reported
/// [`CONTROL_NOT_EXECUTED`].
#[derive(Debug, Clone)]
pub struct AssessmentRecord {
    /// `ResourceId`'s `^[\w\-]+$`, in the `assessment-…` form
    /// `AssessmentIdentifier` accepts back.
    pub id: String,
    pub arn: String,
    /// `VersionedFrameworkId`: `<framework>@<version>`, whose framework is one of
    /// [`ASSESSMENT_FRAMEWORKS`].
    pub framework_id: String,
    /// The framework evaluated, one of [`ASSESSMENT_FRAMEWORKS`] — which
    /// `FrameworkSummary` member the report carries.
    pub framework: String,
    /// The delivery option the framework evaluated, carried by both
    /// `FrameworkSummary` members.
    pub delivery_option_id: Option<String>,
    /// `AssessmentTargetSummary$EntityId`: the entity assessed, if it was one.
    pub entity_id: Option<String>,
    /// `AssessmentTargetSummary$ChangeSetId`: the change set assessed, if it was
    /// one.
    pub change_set_id: Option<String>,
    /// `AssessmentResult`: `PASS` or `FAIL`.
    pub result: String,
    /// `DateTimeISO8601`: `....-..-..T..:..:..Z`.
    pub created_at: String,
    /// `DateTimeISO8601`, the instant the assessment expires.
    pub expires_at: String,
    /// One entry per control the framework evaluated.
    pub controls: Vec<ControlAssessment>,
    /// The instant the RUN this assessment is finishes, for an assessment that
    /// was started and is still running; `None` for one that is finished.
    ///
    /// `StartAssessment` "returns the ID of the assessment immediately" and the
    /// assessment itself "runs asynchronously", so an assessment a caller
    /// started is not finished when the call answers: until this instant the
    /// record reports no `AssessmentResult` and no `ControlAssessments`
    /// ([`Self::reported_result`]), which is what a caller polling
    /// `DescribeAssessment` sees change. The span is
    /// [`ASSESSMENT_RUN`]. An assessment a change set PRODUCED is the recorded
    /// result of applying that change, so it is finished as soon as it exists.
    pub runs_until: Option<SystemTime>,
    /// The tags the assessment carries, held as the SET [`merge_tags`] keeps:
    /// `StartAssessmentRequest$Tags` is "a map of tags to apply to the
    /// assessment when you create it", and the tagging surface reports and
    /// updates them through the assessment's own ARN.
    pub tags: Vec<Tag>,
}

impl AssessmentRecord {
    /// `AssessmentTargetSummary`: which entity or change set was assessed.
    pub fn target_summary(&self) -> AssessmentTargetSummary {
        AssessmentTargetSummary {
            entity_id: self.entity_id.clone(),
            change_set_id: self.change_set_id.clone(),
        }
    }

    /// `FrameworkSummary`: "The set member corresponds to the framework
    /// identified by `FrameworkId`".
    pub fn framework_summary(&self) -> Option<FrameworkSummary> {
        match self.framework.as_str() {
            FRAMEWORK_AMI_SECURITY => Some(FrameworkSummary::AMISecuritySummary(
                AMISecuritySummary {
                    delivery_option_id: self.delivery_option_id.clone(),
                },
            )),
            FRAMEWORK_CONTAINER_SECURITY => Some(FrameworkSummary::ContainerSecuritySummary(
                ContainerSecuritySummary {
                    delivery_option_id: self.delivery_option_id.clone(),
                },
            )),
            _ => None,
        }
    }

    /// Whether the run this assessment is has not finished as of `now`.
    pub fn is_running(&self, now: SystemTime) -> bool {
        self.runs_until.is_some_and(|due| now < due)
    }

    /// The `AssessmentResult` a report carries: the overall result once the run
    /// has finished, and nothing while it is still running — the model
    /// publishes `PASS` and `FAIL` alone, so an assessment that has reached
    /// neither reports no result rather than one it did not establish.
    pub fn reported_result(&self, now: SystemTime) -> Option<String> {
        (!self.is_running(now)).then(|| self.result.clone())
    }

    /// The `ControlAssessments` a report carries: one per control once the run
    /// has finished, and none while it is still running.
    pub fn reported_controls(&self, now: SystemTime) -> Vec<ControlAssessment> {
        if self.is_running(now) {
            return Vec::new();
        }
        self.controls.clone()
    }

    /// The `AssessmentSummary` projection `ListAssessments` reports, as of
    /// `now`.
    pub fn to_summary(&self, now: SystemTime) -> AssessmentSummary {
        AssessmentSummary {
            assessment_arn: Some(self.arn.clone()),
            assessment_id: Some(self.id.clone()),
            framework_id: Some(self.framework_id.clone()),
            assessment_target_summary: Some(self.target_summary()),
            framework_summary: self.framework_summary(),
            assessment_result: self.reported_result(now),
            created_at: Some(self.created_at.clone()),
            expires_at: Some(self.expires_at.clone()),
        }
    }

    /// Whether this assessment was performed against the framework
    /// `framework_id` names — `ListAssessmentsRequest$FrameworkId` is the
    /// UNVERSIONED id ("For example, AMISecurity"), so a request naming one
    /// selects every version of it, and one naming a version selects that
    /// version alone.
    pub fn matches_framework_id(&self, framework_id: &str) -> bool {
        if self.framework_id == framework_id {
            return true;
        }
        self.framework == framework_id
    }

    /// Whether this assessment satisfies an `AssessmentTargetFilter`: "Filters
    /// the list of assessments to those performed against a specific entity or
    /// change set". A member the filter leaves absent constrains nothing.
    ///
    /// `EntityIds` is "a list of up to 50 entity IDs whose assessments you want
    /// to list", so it selects an assessment of ANY entity it names — one
    /// entity's assessments being what each of its entries selects. Which of
    /// those assessments a page then carries is the list's own decision ("the
    /// response includes only the latest published assessment of each entity").
    pub fn matches_target(&self, filter: &AssessmentTargetFilter) -> bool {
        let entity = filter
            .entity_id
            .as_deref()
            .is_none_or(|wanted| self.entity_id.as_deref() == Some(wanted));
        let change_set = filter
            .change_set_id
            .as_deref()
            .is_none_or(|wanted| self.change_set_id.as_deref() == Some(wanted));
        let entities = filter.entity_ids.as_ref().is_none_or(|wanted| {
            self.entity_id
                .as_deref()
                .is_some_and(|held| wanted.iter().any(|asked| asked == held))
        });
        entity && change_set && entities
    }

    /// Whether this assessment satisfies `FrameworkFilters`: "Set exactly one
    /// member to filter results to assessments performed against that
    /// framework" — so the member chosen selects the framework, and the
    /// `DeliveryOptionId` it carries narrows further.
    pub fn matches_framework_filters(&self, filters: &FrameworkFilters) -> bool {
        let (framework, delivery_option) = framework_filters_scope(filters);
        self.framework == framework
            && delivery_option.is_none_or(|wanted| self.delivery_option_id.as_deref() == Some(wanted))
    }
}

/// One page of a list operation's answer, and the token the next page resumes
/// at.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// The page of `all` that starts at `offset` and holds at most `page_size`
/// items, with a `NextToken` only when items are left over — `@paginated`'s
/// contract, and the reason the last page carries none.
pub fn paginate<T>(all: Vec<T>, offset: usize, page_size: usize, prefix: &str) -> Page<T> {
    let total = all.len();
    let items: Vec<T> = all.into_iter().skip(offset).take(page_size).collect();
    let end = offset.saturating_add(items.len());
    Page {
        next_token: (end < total).then(|| encode_page_token(prefix, end)),
        items,
    }
}

/// The page of items a `MaxResults` asks for, or the modelled
/// `ValidationException` that refuses it.
///
/// Every paginated operation of this service models `MaxResults` with a
/// `@range(min: 1, max: <limit>)` and documents the page it serves when the
/// request names none, so a requested size inside that range is the size served,
/// one outside it is refused with the sentence this service states an
/// out-of-bounds page size in — naming the size asked for and the range the
/// member declares — and a request naming none serves that documented default,
/// which is the maximum only where the operation publishes the same number for
/// both. This is the one place that decision is made, so each paginated
/// operation judges a page size the same way: `default` is the page its own
/// member documents for an absent `MaxResults` and `limit` the maximum its own
/// range declares.
///
/// The range is judged HERE rather than by the boundary walk (`MaxResults`
/// carries `skip_validation` on every paginated input of this service): a
/// boundary refusal takes the protocol's status, where this service answers the
/// modelled `ValidationException` and its 422 for a member outside its
/// constraint.
///
/// `requested` is `None` for the operations whose `MaxResults` is optional and
/// carries no `@default`, so the handler owns the default; the ones that model a
/// `@default` are handed the value the boundary already applied.
pub fn page_size(
    default: i32,
    limit: i32,
    requested: Option<i32>,
) -> Result<usize, ValidationException> {
    let serving = match requested {
        None => default,
        Some(requested) if (1..=limit).contains(&requested) => requested,
        Some(requested) => return Err(page_size_out_of_bounds(requested, limit)),
    };
    Ok(usize::try_from(serving).expect("a page size inside the modelled range fits in usize"))
}

/// The modelled `ValidationException` payload for a `MaxResults` outside the
/// range its member declares — built from the request, naming the size asked for
/// and the bounds that member publishes.
pub fn page_size_out_of_bounds(requested: i32, limit: i32) -> ValidationException {
    validation_message(format!(
        "Provided max results value '{requested}' is out of bounds. It should be \
         between 1 and {limit}."
    ))
}

/// The offset a `NextToken` asks the list paging with `prefix` to resume at, or
/// the modelled `ValidationException` that refuses it.
///
/// The page-TOKEN decision, the sibling of [`page_size`]'s page-size decision
/// and made in this one place for the same reason: a request naming no
/// `NextToken` asks for the first page (offset zero), one carrying a token this
/// service issued for the list being called resumes where that token points, and
/// anything else — a token outside the modelled `NextToken` syntax, a well-formed
/// token naming no page, or a token another list of this service issued — names
/// no page of this list and is the one [`invalid_page_token`] fault.
pub fn page_offset(prefix: &str, token: Option<&str>) -> Result<usize, ValidationException> {
    match token {
        None => Ok(0),
        Some(token) => decode_page_token(prefix, token).ok_or_else(invalid_page_token),
    }
}

/// Per-service state for com-amazonaws-marketplacecatalog-awsmpseymour. A handler reaches it through [`service_state`].
///
/// There is one store rather than one per catalog because every operation's
/// `Catalog` member documents a fixed value ([`AWS_MARKETPLACE_CATALOG`]), which
/// the handlers refuse anything else for.
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacecatalogAwsmpseymourState {
    /// Every change set, keyed by its id. A `BTreeMap` is what gives the lists
    /// a stable total order to sort and page within — ids sort in the order
    /// they were minted.
    pub change_sets: BTreeMap<String, ChangeSetRecord>,
    /// Every entity a succeeded `APPLY` change set has created, keyed by id.
    pub entities: BTreeMap<String, EntityRecord>,
    /// Every assessment, keyed by its id. A succeeded `Intent=APPLY` change set
    /// that submits a delivery option of a product one of this catalog's
    /// frameworks covers is what creates one — see [`AssessmentRecord`].
    pub assessments: BTreeMap<String, AssessmentRecord>,
    /// A `StartChangeSet` `ClientRequestToken` to the change set that token
    /// started, so a retried start reports the first change set instead of
    /// minting a second.
    pub idempotency_tokens: HashMap<String, String>,
    /// The same for a `StartAssessment` `ClientToken` and the assessment it
    /// started: a separate namespace, because a token names the request it was
    /// sent with and the two operations start different resources.
    pub assessment_tokens: HashMap<String, String>,
    /// The counter change set ids are minted from.
    pub change_set_seq: u64,
    /// The counter entity ids are minted from.
    pub entity_seq: u64,
    /// The counter assessment ids are minted from.
    pub assessment_seq: u64,
    /// The counter the delivery option ids this catalog assigns are minted
    /// from.
    pub delivery_option_seq: u64,
    /// The resource-based policy attached to each resource that has one, keyed
    /// by [`CatalogResource::policy_key`] — the resource's type and id, which
    /// is what identifies it whatever region or partition the ARN naming it was
    /// written with. A resource absent from the map has no policy attached.
    ///
    /// Only an ENTITY resource is ever a key here: a resource-based policy is
    /// attached to, reported for and cleared on an entity
    /// ([`Self::policy_key_for`]).
    pub resource_policies: BTreeMap<String, String>,
}

impl ComAmazonawsMarketplacecatalogAwsmpseymourState {
    /// Mint an unused change set id matching `ResourceId`'s `^[\w\-]+$`.
    pub fn mint_change_set_id(&mut self) -> String {
        loop {
            self.change_set_seq = self.change_set_seq.wrapping_add(1);
            let id = format!("cs-{:012x}", self.change_set_seq & 0xffff_ffff_ffff);
            if !self.change_sets.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an unused entity id matching `ResourceId`'s `^[\w\-]+$`.
    ///
    /// The loop also skips an id a running change set has already reserved, so
    /// two change sets in flight never name the same new entity.
    pub fn mint_entity_id(&mut self) -> String {
        loop {
            self.entity_seq = self.entity_seq.wrapping_add(1);
            let id = format!("ent-{:012x}", self.entity_seq & 0xffff_ffff_ffff);
            if !self.entities.contains_key(&id) && self.locking_change_set(&id).is_none() {
                return id;
            }
        }
    }

    /// Mint an unused assessment id, in the `assessment-…` form
    /// `AssessmentIdentifier` accepts back ([`is_assessment_id`]).
    pub fn mint_assessment_id(&mut self) -> String {
        loop {
            self.assessment_seq = self.assessment_seq.wrapping_add(1);
            let id = format!(
                "{ASSESSMENT_ID_PREFIX}{:012x}",
                self.assessment_seq & 0xffff_ffff_ffff
            );
            if !self.assessments.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a delivery option id for an option a change submitted without one,
    /// matching `ResourceId`'s `^[\w\-]+$`.
    pub fn mint_delivery_option_id(&mut self) -> String {
        self.delivery_option_seq = self.delivery_option_seq.wrapping_add(1);
        format!(
            "{DELIVERY_OPTION_ID_PREFIX}{:012x}",
            self.delivery_option_seq & 0xffff_ffff_ffff
        )
    }

    /// The id of the still-running change set that has `entity_id` locked, if
    /// any.
    ///
    /// `StartChangeSet` documents the lock: "when a ChangeSet is running, all
    /// the entities targeted by the different changes are locked until the
    /// change set has completed (either succeeded, cancelled, or failed)".
    pub fn locking_change_set(&self, entity_id: &str) -> Option<&str> {
        self.change_sets
            .values()
            .find(|record| {
                record.is_running()
                    && record
                        .changes
                        .iter()
                        .any(|change| change.entity_id == entity_id)
            })
            .map(|record| record.id.as_str())
    }

    /// The entity `identifier` names, where an identifier is an entity id with
    /// an optional `@<revision>` suffix (`Identifier`'s `^[\w\-@]+$`).
    pub fn resolve_identifier(&self, identifier: &str) -> Option<&EntityRecord> {
        self.entities.get(entity_id_of(identifier))
    }

    /// Whether this store holds the resource a resource ARN named.
    ///
    /// An entity's ARN names its `EntityType` as well as its id, so a record
    /// whose type is not the one the ARN carries is NOT the resource that ARN
    /// names.
    pub fn holds_resource(&self, resource: &CatalogResource) -> bool {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .is_some_and(|held| &held.entity_type == entity_type),
            CatalogResource::ChangeSet { id } => self.change_sets.contains_key(id),
            CatalogResource::Assessment { id } => self.assessments.contains_key(id),
        }
    }

    /// The key [`Self::resource_policies`] holds the resource-based policy of
    /// the resource a resource ARN named under, or `None` when that ARN names no
    /// resource a resource-based policy can be attached to, reported for and
    /// cleared on.
    ///
    /// Such a resource is an ENTITY: each resource-policy operation is
    /// documented for one, and names its `ResourceArn` "the ARN of the entity
    /// resource". So an ARN naming a resource of this catalog of any OTHER kind
    /// — [`CHANGE_SET_RESOURCE_TYPE`] is the one the catalog has besides its
    /// entities, and the tagging surface is what is documented for "either an
    /// entity or change set" — names no resource of this surface at all, as
    /// does an ARN carrying a resource type this catalog does not have
    /// ([`ENTITY_TYPES`] is every entity type one of its ARNs names).
    ///
    /// A policy is recorded per resource in [`Self::resource_policies`] rather
    /// than on this catalog's own record of the resource, and the records here
    /// are only the resources this emulator's own change sets created — an
    /// account's catalog holds entities no change set of this run created — so
    /// absence from them is not evidence that an ARN names no resource. What IS
    /// such evidence: an id this catalog holds a record of under a DIFFERENT
    /// resource kind or entity type, which the ARN then contradicts.
    pub fn policy_key_for(&self, resource: &CatalogResource) -> Option<String> {
        let CatalogResource::Entity { entity_type, id } = resource else {
            return None;
        };
        let names_an_entity = is_entity_type(entity_type)
            // An id this catalog holds as a CHANGE SET is not an entity of any
            // type, whatever type the ARN carries.
            && !self.change_sets.contains_key(id)
            && self
                .entities
                .get(id)
                .is_none_or(|held| &held.entity_type == entity_type);
        if !names_an_entity {
            return None;
        }
        resource.policy_key()
    }

    /// The ARN the stored record carries for the resource a resource ARN named,
    /// which is what an operation reporting the resource reports — never the
    /// ARN the request was written with.
    pub fn stored_resource_arn(&self, resource: &CatalogResource) -> Option<&str> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| held.arn.as_str()),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get(id).map(|held| held.arn.as_str())
            }
            CatalogResource::Assessment { id } => {
                self.assessments.get(id).map(|held| held.arn.as_str())
            }
        }
    }

    /// The tags the stored record carries for the resource a resource ARN
    /// named: the one copy `StartChangeSet` wrote and `TagResource` updates,
    /// held as the SET [`merge_tags`] keeps — one value per key.
    pub fn resource_tags(&self, resource: &CatalogResource) -> Option<&[Tag]> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| held.tags.as_slice()),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get(id).map(|held| held.tags.as_slice())
            }
            CatalogResource::Assessment { id } => {
                self.assessments.get(id).map(|held| held.tags.as_slice())
            }
        }
    }

    /// Those same tags, for writing — beside the instant the record reports as
    /// its last modification, so a tag write restamps the record it modifies
    /// ([`ResourceTagsWrite::store`]).
    pub fn resource_tags_write(
        &mut self,
        resource: &CatalogResource,
    ) -> Option<ResourceTagsWrite<'_>> {
        match resource {
            CatalogResource::Entity { entity_type, id } => self
                .entities
                .get_mut(id)
                .filter(|held| &held.entity_type == entity_type)
                .map(|held| ResourceTagsWrite {
                    tags: &mut held.tags,
                    last_modified_date: Some(&mut held.last_modified_date),
                }),
            CatalogResource::ChangeSet { id } => {
                self.change_sets.get_mut(id).map(|held| ResourceTagsWrite {
                    tags: &mut held.tags,
                    // A change set reports `StartTime` and `EndTime`, the
                    // instants of the run it is: it publishes no member saying
                    // when it was last modified, so tagging one moves no
                    // timestamp of it.
                    last_modified_date: None,
                })
            }
            CatalogResource::Assessment { id } => {
                self.assessments.get_mut(id).map(|held| ResourceTagsWrite {
                    tags: &mut held.tags,
                    // An assessment reports `CreatedAt` and `ExpiresAt`, the
                    // instants of the evaluation it is: like a change set it
                    // publishes no last-modified member, so tagging one moves
                    // no timestamp of it.
                    last_modified_date: None,
                })
            }
        }
    }

    /// Materialise every change the clock owes: a change set past its due
    /// instant moves on, and the transition that ends it is the one that
    /// applies its changes — which either writes the entities they describe
    /// (`Intent=APPLY`) or fails the change set on what they asked for.
    ///
    /// Each change is stamped with the instant it fell DUE, never with `now`,
    /// so what a caller reads does not depend on when it read.
    pub fn settle(&mut self, now: SystemTime) {
        let ids: Vec<String> = self.change_sets.keys().cloned().collect();
        for id in ids {
            while let Some(record) = self.change_sets.get(&id) {
                let Some(next) = record.schedule.first() else {
                    break;
                };
                if now < next.due {
                    break;
                }
                let (due, status) = (next.due, next.status.clone());
                // The terminal transition is where the changes are APPLIED, so
                // the status it lands on is the OUTCOME of applying them: the
                // schedule asks for the end of the run, not for a success.
                let verdict = if status == STATUS_SUCCEEDED {
                    self.judge_change_set(&id)
                } else {
                    ChangeSetVerdict::default()
                };
                let Some(record) = self.change_sets.get_mut(&id) else {
                    break;
                };
                record.schedule.remove(0);
                if status != STATUS_SUCCEEDED {
                    record.status = status;
                    continue;
                }
                record.end_time = Some(iso8601(due));
                if verdict.failed() {
                    record.status = STATUS_FAILED.to_string();
                    record.failure_code = Some(FAILURE_CODE_CLIENT_ERROR.to_string());
                    record.failure_description = verdict.failure_description;
                    for (change, errors) in record.changes.iter_mut().zip(verdict.change_errors) {
                        change.errors = errors;
                    }
                    // A change set that failed applied nothing: the entities
                    // its changes described are not written, and the ones they
                    // named are left as they were.
                    continue;
                }
                record.status = status;
                if record.intent == INTENT_APPLY {
                    self.apply_changes(&id, due);
                }
            }
        }
    }

    /// Judge the changes of change set `id` the way applying them does, without
    /// writing anything.
    ///
    /// A change's details are an opaque JSON document — `Change$Details` is a
    /// `Json` string and `Change$DetailsDocument` "accepts a JSON value instead
    /// of a string for `ChangeType` details", and the model declares no shape
    /// for what is inside either — so a defect among the members the catalog
    /// READS is one only applying can find. Applying is also the first point
    /// that knows what the entity already holds, which is what the tag bound is
    /// judged against.
    pub fn judge_change_set(&self, id: &str) -> ChangeSetVerdict {
        let Some(record) = self.change_sets.get(id) else {
            return ChangeSetVerdict::default();
        };
        let mut verdict = ChangeSetVerdict {
            change_errors: vec![Vec::new(); record.changes.len()],
            failure_description: None,
        };
        // The tag keys each entity this change set touches would end up
        // holding, carried across the changes in submission order the way
        // applying merges them — so the change that crosses the bound is the
        // one that reports it. A SET, so merging one change's tags costs one
        // lookup per tag rather than a scan of the keys already held.
        let mut tag_keys: HashMap<String, HashSet<String>> = HashMap::new();
        // The details each entity this change set touches would end up holding,
        // carried across the changes the same way, for the same reason: the
        // merge is what a `Json` member has to report afterwards, so the change
        // that takes it past the bound is the one that reports it.
        let mut entity_details: HashMap<String, Map<String, Value>> = HashMap::new();
        for (index, change) in record.changes.iter().enumerate() {
            let details = change.details_value();
            verdict.change_errors[index] =
                details_defects(entity_type_of(&change.entity_type), details.as_ref());
            // Every change resolved to an entity, whatever the intent, so every
            // change is judged against what that entity would then hold: a
            // validation reaches the verdict applying the same request reaches.
            let entity_id = change.entity_id.as_str();
            let merged = entity_details.entry(entity_id.to_string()).or_insert_with(|| {
                self.entities
                    .get(entity_id)
                    .and_then(|entity| entity.details.as_object().cloned())
                    .unwrap_or_default()
            });
            if let Some(members) = details.as_ref().and_then(Value::as_object) {
                for (key, member) in members {
                    merged.insert(key.clone(), member.clone());
                }
            }
            // What the entity would report: the merge, plus the ids applying
            // assigns to the delivery options it carries that have none yet.
            let length = details_json_length(merged).saturating_add(applied_details_growth(merged));
            if length > DETAILS_JSON_LIMIT {
                verdict.change_errors[index].push(ErrorDetail {
                    error_code: Some(ERROR_CODE_LIMIT_EXCEEDED.to_string()),
                    error_message: Some(format!(
                        "Details: an entity holds at most {DETAILS_JSON_LIMIT} characters of \
                         details, and this change would leave {entity_id} with {length}"
                    )),
                });
            }
            let held = tag_keys.entry(entity_id.to_string()).or_insert_with(|| {
                self.entities
                    .get(entity_id)
                    .map(|entity| entity.tags.iter().map(|tag| tag.key.clone()).collect())
                    .unwrap_or_default()
            });
            for tag in &change.entity_tags {
                held.insert(tag.key.clone());
            }
            if held.len() > MAX_RESOURCE_TAGS {
                let count = held.len();
                verdict.change_errors[index].push(ErrorDetail {
                    error_code: Some(ERROR_CODE_LIMIT_EXCEEDED.to_string()),
                    error_message: Some(format!(
                        "EntityTags: an entity holds at most {MAX_RESOURCE_TAGS} tags, and this \
                         change would leave {entity_id} with {count}"
                    )),
                });
            }
        }
        if verdict.failed() {
            return verdict;
        }
        // `FailureDescription` is "Returned if there is a failure on the change
        // set, but that failure is not related to any of the changes in the
        // request": every change above is applicable on its own, so a failure
        // left here belongs to the set rather than to one of its changes.
        verdict.failure_description = contradicting_details(&record.changes);
        verdict
    }

    /// Write the entities the changes of change set `id` describe, as of `at`,
    /// and record what evaluating those entities against this catalog's
    /// frameworks found.
    ///
    /// A change naming an existing entity mutates that record and bumps its
    /// revision; a change naming none creates the record under the id
    /// `StartChangeSet` reserved for it.
    ///
    /// Applying a change is also where the catalog EVALUATES what the change
    /// submitted: a delivery option of a product one of its security frameworks
    /// covers ([`entity_framework`]) is assessed against that framework, which
    /// is what `ListAssessments` then reports and `DescribeAssessment` details.
    fn apply_changes(&mut self, id: &str, at: SystemTime) {
        let Some(record) = self.change_sets.get(id) else {
            return;
        };
        let changes = record.changes.clone();
        let region = record.region.clone();
        let change_set_id = record.id.clone();
        let stamp = iso8601(at);
        for change in changes {
            let entity_id = change.entity_id.clone();
            let mut details = change.details_value();
            // `Change$Entity$Type` is a REFERENCE to an entity type at a schema
            // version (`DataProduct@1.0`); the entity itself is of the type that
            // reference names, so the record's type and its ARN carry the type
            // alone, and the version the change addressed it at is kept beside
            // them for the reads whose members report the type at it.
            let entity_type = entity_type_of(&change.entity_type).to_string();
            let entity_type_version = entity_type_version_of(&change.entity_type).map(str::to_string);
            // A delivery option this change submitted without an id is given
            // one BEFORE the entity is written, so the entity carries the same
            // id the assessment of that option reports.
            let options = self.identify_delivery_options(details.as_mut());
            {
                let entry = self.entities.entry(entity_id.clone()).or_insert_with(|| {
                    EntityRecord {
                        arn: entity_arn(&region, &entity_type, &entity_id),
                        id: entity_id.clone(),
                        entity_type: entity_type.clone(),
                        entity_type_version: entity_type_version.clone(),
                        revision: 0,
                        name: None,
                        visibility: DEFAULT_ENTITY_VISIBILITY.to_string(),
                        last_modified_date: stamp.clone(),
                        details: Value::Object(Map::new()),
                        tags: Vec::new(),
                    }
                });
                entry.revision = entry.revision.saturating_add(1);
                entry.last_modified_date = stamp.clone();
                // The schema version moves with the changes, as the
                // last-modified instant does: the entity is at the version the
                // change that last wrote it addressed its type through.
                if entity_type_version.is_some() {
                    entry.entity_type_version = entity_type_version.clone();
                }
                merge_tags(&mut entry.tags, change.entity_tags);
                if let Some(Value::Object(members)) = details {
                    if let Some(name) = detail_name(&entity_type, &members) {
                        entry.name = Some(name);
                    }
                    if let Some(visibility) = detail_visibility(&entity_type, &members) {
                        entry.visibility = visibility;
                    }
                    if let Value::Object(held) = &mut entry.details {
                        for (key, value) in members {
                            held.insert(key, value);
                        }
                    }
                }
            }
            // "An assessment is the result of evaluating a product or change
            // set against a framework": the product is the entity this change
            // applied to, so a change against an entity type no framework of
            // this catalog covers is evaluated against nothing, and so is one
            // that submitted no delivery option for a framework to assess.
            let Some(framework) = entity_framework(&entity_type) else {
                continue;
            };
            for option in &options {
                self.record_assessment(framework, &region, &entity_id, &change_set_id, option, at);
            }
        }
    }

    /// Assign an id to every delivery option `details` submits without one, and
    /// report those options as the entity now carries them.
    ///
    /// A delivery option is addressed by an id the CATALOG assigns — the id
    /// `FrameworkSummary$DeliveryOptionId` names and
    /// `AMISecurityFilters$DeliveryOptionId` selects by — so an option
    /// submitted without one is given an id here, which is then both what the
    /// entity's details carry and what the assessment of that option reports.
    /// An option submitted WITH an id keeps it: the change is updating the
    /// option that id names.
    ///
    /// The list is bounded at [`MAX_DELIVERY_OPTIONS_PER_CHANGE`], which
    /// `StartChangeSet` refuses a change past, so the work here and the number
    /// of assessments it leads to are bounded by that constant and not by the
    /// width of the submitted document. The bound is re-applied here so it
    /// dominates the records written below whatever reaches the store.
    fn identify_delivery_options(&mut self, details: Option<&mut Value>) -> Vec<Value> {
        let Some(Value::Array(options)) =
            details.and_then(|held| held.get_mut(DELIVERY_OPTIONS_DETAIL_MEMBER))
        else {
            return Vec::new();
        };
        let mut identified: Vec<Value> = Vec::with_capacity(
            options.len().min(MAX_DELIVERY_OPTIONS_PER_CHANGE),
        );
        for option in options.iter_mut().take(MAX_DELIVERY_OPTIONS_PER_CHANGE) {
            if !option.is_object() {
                continue;
            }
            if option
                .get(DELIVERY_OPTION_ID_MEMBER)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                let minted = self.mint_delivery_option_id();
                if let Value::Object(members) = option {
                    members.insert(DELIVERY_OPTION_ID_MEMBER.to_string(), Value::String(minted));
                }
            }
            identified.push(option.clone());
        }
        identified
    }

    /// Record what evaluating `option` against `framework` found: one
    /// assessment of the entity `entity_id`, submitted by change set
    /// `change_set_id`, created as of `at`.
    ///
    /// The overall `AssessmentResult` is `FAIL` exactly when one of the
    /// framework's controls failed — `PASS` says that none did, never that
    /// every control ran.
    fn record_assessment(
        &mut self,
        framework: &str,
        region: &str,
        entity_id: &str,
        change_set_id: &str,
        option: &Value,
        at: SystemTime,
    ) {
        let controls = assess_delivery_option(framework, option);
        if controls.is_empty() {
            return;
        }
        let id = self.mint_assessment_id();
        let record = AssessmentRecord {
            arn: assessment_arn(region, &id),
            id,
            framework_id: format!("{framework}@{ASSESSMENT_FRAMEWORK_VERSION}"),
            framework: framework.to_string(),
            delivery_option_id: option
                .get(DELIVERY_OPTION_ID_MEMBER)
                .and_then(Value::as_str)
                .map(str::to_string),
            entity_id: Some(entity_id.to_string()),
            change_set_id: Some(change_set_id.to_string()),
            result: assessment_result(&controls).to_string(),
            created_at: iso8601(at),
            expires_at: iso8601(at + ASSESSMENT_VALIDITY),
            controls,
            // The record IS the result of applying the change, so this
            // assessment is finished as soon as it exists: nothing of it is
            // still running.
            runs_until: None,
            // A change set applies tags to the ENTITIES its changes name; the
            // assessment it produced carries none until one is put on it.
            tags: Vec::new(),
        };
        self.store_assessment(record);
    }

    /// Store `record` under its id, making room for it first.
    ///
    /// The store holds at most [`ASSESSMENT_QUOTA`] assessments: at the bound
    /// the OLDEST is dropped to make room for this one, so the state the
    /// assessment surface holds is bounded however many assessments are
    /// produced or started.
    pub fn store_assessment(&mut self, record: AssessmentRecord) {
        while self.assessments.len() >= ASSESSMENT_QUOTA {
            let Some(oldest) = self
                .assessments
                .values()
                .min_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then_with(|| left.id.cmp(&right.id))
                })
                .map(|record| record.id.clone())
            else {
                break;
            };
            self.assessments.remove(&oldest);
        }
        self.assessments.insert(record.id.clone(), record);
    }

    /// Remember that the `StartAssessment` token `token` started the assessment
    /// `id`, so a retried start reports that assessment instead of starting a
    /// second.
    ///
    /// The map is bounded the way the records it points at are
    /// ([`ASSESSMENT_QUOTA`]): at the bound every token naming an assessment
    /// the store no longer holds is dropped, since such a token names nothing a
    /// retry could report.
    pub fn remember_assessment_token(&mut self, token: String, id: String) {
        if self.assessment_tokens.len() >= ASSESSMENT_QUOTA {
            let held = &self.assessments;
            self.assessment_tokens
                .retain(|_, named| held.contains_key(named));
        }
        self.assessment_tokens.insert(token, id);
    }

    /// The assessment of the entity `entity_id` that is still running as of    /// `now`, if one is.
    ///
    /// An assessment is a run over one entity, so the entity is in use for as
    /// long as that run lasts: this is what a second start against the same
    /// entity is refused on ([`assessment_in_progress`]).
    pub fn running_assessment_of(&self, entity_id: &str, now: SystemTime) -> Option<&str> {
        self.assessments
            .values()
            .find(|record| {
                record.entity_id.as_deref() == Some(entity_id) && record.is_running(now)
            })
            .map(|record| record.id.as_str())
    }

    /// How many assessments of this account are still running as of `now` —
    /// the open requests [`OPEN_ASSESSMENT_LIMIT`] bounds.
    pub fn open_assessments(&self, now: SystemTime) -> usize {
        self.assessments
            .values()
            .filter(|record| record.is_running(now))
            .count()
    }
}

/// The `AssessmentResult` a set of control results adds up to: `FAIL` exactly
/// when one of the controls failed.
///
/// `PASS` says that none did, never that every control ran — a control the
/// assessed resource carries nothing for is [`CONTROL_NOT_EXECUTED`] and fails
/// nothing.
pub fn assessment_result(controls: &[ControlAssessment]) -> &'static str {
    let failed = controls
        .iter()
        .any(|control| control.control_assessment_result.as_deref() == Some(CONTROL_FAIL));
    if failed {
        ASSESSMENT_FAIL
    } else {
        ASSESSMENT_PASS
    }
}

/// The modelled `ValidationException` payload for a change set carrying more
/// changes than [`MAX_CHANGES_PER_CHANGE_SET`] — built from the request: the
/// count it carried and the bound it passed.
pub fn too_many_changes(count: usize) -> ValidationException {
    validation(
        "ChangeSet",
        format!(
            "a change set carries at most {MAX_CHANGES_PER_CHANGE_SET} changes, not {count}"
        ),
    )
}

/// The modelled `ValidationException` payload for a change submitting more
/// delivery options than [`MAX_DELIVERY_OPTIONS_PER_CHANGE`] — built from the
/// request: the count it carried and the bound it passed.
pub fn too_many_delivery_options(count: usize) -> ValidationException {
    validation(
        "ChangeSet.Details",
        format!(
            "a change submits at most {MAX_DELIVERY_OPTIONS_PER_CHANGE} delivery options, \
             not {count}"
        ),
    )
}

/// The `Name` the details of an applied change give an entity of
/// `entity_type`, if any: the value they carry at the one member that type
/// names an entity's name ([`entity_name_detail_member`]) — never a member
/// another type names it, which this entity's summary reports nothing from.
///
/// Only an applied change set reaches here, and a name outside
/// [`entity_name_contract`] fails the change set before it is applied
/// ([`details_defects`]), so nothing a summary member cannot hold reaches a
/// record — the same two-sided rule [`detail_visibility`] follows.
fn detail_name(entity_type: &str, members: &Map<String, Value>) -> Option<String> {
    let contract = entity_name_contract(entity_type);
    members
        .get(entity_name_detail_member(entity_type))?
        .as_str()
        .filter(|text| contract.admits_text(text))
        .map(str::to_string)
}

/// The `Visibility` the details of an applied change give an entity of
/// `entity_type`, if any.
///
/// Only an applied change set reaches here, and a value that type's own enum
/// does not publish ([`entity_visibility_contract`]) fails the change set
/// before it is applied ([`details_defects`]), so nothing outside the set the
/// model declares for that type reaches a record.
fn detail_visibility(entity_type: &str, members: &Map<String, Value>) -> Option<String> {
    let value = members.get(ENTITY_VISIBILITY_DETAIL_MEMBER)?.as_str()?;
    entity_visibility_contract(entity_type)
        .admits_text(value)
        .then(|| value.to_string())
}

/// What applying a change set found: the `ErrorDetailList` of each of its
/// changes, and a failure of the change set that belongs to none of them.
#[derive(Debug, Default, Clone)]
pub struct ChangeSetVerdict {
    /// One entry per change, in submission order. A change applying found
    /// nothing wrong with carries an empty one.
    pub change_errors: Vec<Vec<ErrorDetail>>,
    /// `FailureDescription`, for a failure "not related to any of the changes
    /// in the request".
    pub failure_description: Option<String>,
}

impl ChangeSetVerdict {
    /// True when the change set cannot be applied, so the end of its run is
    /// `FAILED` rather than `SUCCEEDED`.
    pub fn failed(&self) -> bool {
        self.failure_description.is_some()
            || self.change_errors.iter().any(|errors| !errors.is_empty())
    }
}

/// The characters the `@pattern` of a shape a summary member targets admits.
///
/// A pattern is a statement about the FORM of a value, so each variant is one
/// such form, named after the shapes that declare it; a value is judged against
/// every form the members reporting it declare, beside the `@length` of their
/// targets ([`DetailContract`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailSyntax {
    /// `^(.)+$`, the pattern of every title, name and free-text member of the
    /// per-type summaries: one or more characters, none of them a line break
    /// (`.` matches no line terminator).
    OneLine,
    /// `EntityNameString`'s `^\S+[\S\s]*$`: a value whose first character is
    /// not whitespace.
    NonBlankStart,
    /// `^\d{12}$`, the pattern of every account-id member: exactly that many
    /// digits, the count being the `@length` of the shape that declares it.
    Digits(usize),
    /// [`ENTITY_ID_PATTERN`], the pattern of every id member a seller or the
    /// catalog assigns.
    Id,
    /// The same form behind a fixed prefix, as `OfferSetIdString`'s
    /// `^offerset-[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$` declares it.
    PrefixedId(&'static str),
    /// A fixed prefix followed by exactly that many alphanumerics, as
    /// `OfferSubsidiaryAuthorizationIdString`'s
    /// `^subauthz-[a-zA-Z0-9]{13}$` declares it.
    PrefixedCode(&'static str, usize),
    /// `DateTimeISO8601`'s pattern: the 20-character instant [`is_iso8601`]
    /// judges.
    Instant,
}

impl DetailSyntax {
    /// Whether `value` takes this form.
    fn admits(self, value: &str) -> bool {
        match self {
            Self::OneLine => !value.is_empty() && !value.chars().any(is_line_break),
            Self::NonBlankStart => value.chars().next().is_some_and(|c| !c.is_whitespace()),
            Self::Digits(_) => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
            Self::Id => is_entity_id(value),
            Self::PrefixedId(prefix) => value.strip_prefix(prefix).is_some_and(is_entity_id),
            Self::PrefixedCode(prefix, count) => value
                .strip_prefix(prefix)
                .is_some_and(|code| {
                    code.chars().count() == count && code.chars().all(|c| c.is_ascii_alphanumeric())
                }),
            Self::Instant => is_iso8601(value),
        }
    }

    /// The `@pattern` this form is, as the model writes it — which is what a
    /// refusal quotes back, as every other syntax this service refuses a value
    /// against is quoted back ([`ENTITY_ID_PATTERN`]).
    fn pattern(self) -> String {
        match self {
            Self::OneLine => "^(.)+$".to_string(),
            Self::NonBlankStart => r"^\S+[\S\s]*$".to_string(),
            Self::Digits(count) => format!(r"^\d{{{count}}}$"),
            Self::Id => ENTITY_ID_PATTERN.to_string(),
            Self::PrefixedId(prefix) => {
                format!("^{prefix}{}", ENTITY_ID_PATTERN.trim_start_matches('^'))
            }
            Self::PrefixedCode(prefix, count) => {
                format!("^{prefix}[a-zA-Z0-9]{{{count}}}$")
            }
            Self::Instant => DATE_TIME_PATTERN.to_string(),
        }
    }
}

/// Whether `c` is a line terminator — a character `.` does not match, which is
/// what makes `^(.)+$` a single-line value.
fn is_line_break(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// `text` as a single-line value carries it: every line terminator
/// ([`is_line_break`]) written as the escape sequence that stands for it, and
/// every other character left as sent.
///
/// A sentence this service builds around a SUBMITTED value is carried by a
/// member whose shape declares `@pattern("^(.)+$")` —
/// `ExceptionMessageContent` on a `ValidationException$Message` and an
/// `ErrorDetail$ErrorMessage`, `BatchDescribeErrorMessageContent` on a
/// `BatchDescribeErrorDetail$ErrorMessage` — which admits no line terminator,
/// so a value carrying one is quoted ESCAPED rather than spliced in raw. The
/// escapes are the ones JSON writes, which is how [`quoted`] already renders
/// the value it quotes, so both quoting paths show the same value the same way.
fn one_line(text: &str) -> String {
    if !text.chars().any(is_line_break) {
        return text.to_string();
    }
    let mut single = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => single.push_str("\\n"),
            '\r' => single.push_str("\\r"),
            c if is_line_break(c) => single.push_str(&format!("\\u{:04x}", c as u32)),
            c => single.push(c),
        }
    }
    single
}

/// `DateTimeISO8601`'s `@pattern`, quoted in a refusal of a value outside it.
pub const DATE_TIME_PATTERN: &str = r"^([\d]{4})\-(1[0-2]|0[1-9])\-(3[01]|0[1-9]|[12][\d])T(2[0-3]|[01][\d]):([0-5][\d]):([0-5][\d])Z$";

/// What one details member may hold, as the model declares it on the
/// `EntitySummary` member that REPORTS that value: the enum's values, or the
/// `@length` and `@pattern`s of the string shapes it targets.
#[derive(Debug, Clone, Copy)]
pub struct DetailContract {
    /// The values the target ENUMERATES, and `&[]` when the target is a string
    /// shape bounded by a length and a pattern instead.
    values: &'static [&'static str],
    /// The target's `@length(min, max)`, in characters. Unused for an
    /// enumerated member, whose contract is its value set.
    length: (usize, usize),
    /// Every `@pattern` the value satisfies: the one its own target declares,
    /// and the one of any other member reporting the same value.
    syntax: &'static [DetailSyntax],
    /// `Some(max)` when the summary member is a LIST of such values, carrying
    /// that list's own `@length(max)`.
    items: Option<usize>,
}

impl DetailContract {
    /// Whether `value`, as a details member holds it, is one this member may
    /// report: a list member one list of admitted values, and every other
    /// member one admitted string.
    fn admits(&self, value: &Value) -> bool {
        match self.items {
            Some(max) => value.as_array().is_some_and(|items| {
                items.len() <= max
                    && items
                        .iter()
                        .all(|item| item.as_str().is_some_and(|text| self.admits_text(text)))
            }),
            None => value.as_str().is_some_and(|text| self.admits_text(text)),
        }
    }

    /// Whether the string `value` is one this member may report.
    fn admits_text(&self, value: &str) -> bool {
        if !self.values.is_empty() {
            return self.values.contains(&value);
        }
        let (min, max) = self.length;
        (min..=max).contains(&value.chars().count())
            && self.syntax.iter().all(|form| form.admits(value))
    }

    /// What this member accepts, as a refusal states it: the published set, or
    /// the length and the pattern(s) the value has to satisfy.
    fn accepts(&self) -> String {
        match self.items {
            Some(max) => format!(
                "a list of at most {max} values, each {}",
                self.accepts_value()
            ),
            None => self.accepts_value(),
        }
    }

    /// The same sentence for ONE value of the member, which is the whole of it
    /// for every member that is not a list.
    fn accepts_value(&self) -> String {
        if !self.values.is_empty() {
            return format!("one of {:?}", self.values);
        }
        let (min, max) = self.length;
        let length = if min == max {
            format!("{min} characters")
        } else {
            format!("{min} to {max} characters")
        };
        if self.syntax.is_empty() {
            // A member the model bounds by `@length` alone ([`bounded`]) has no
            // syntax to state.
            return length;
        }
        let patterns: Vec<String> = self.syntax.iter().map(|form| form.pattern()).collect();
        format!("{length} matching {}", patterns.join(" and "))
    }
}

/// The contract of a member whose target ENUMERATES its values.
const fn choice(values: &'static [&'static str]) -> DetailContract {
    DetailContract {
        values,
        length: (0, 0),
        syntax: &[],
        items: None,
    }
}

/// The contract of a member targeting a string of one to `max` characters on a
/// single line (`@length(min: 1, max)` with `@pattern("^(.)+$")`).
const fn text(max: usize) -> DetailContract {
    DetailContract {
        values: &[],
        length: (1, max),
        syntax: &[DetailSyntax::OneLine],
        items: None,
    }
}

/// The contract of a member targeting a string the model bounds by LENGTH
/// alone: `@length(min: 1, max)` with no `@pattern` beside it, so the value's
/// characters are unconstrained.
const fn bounded(max: usize) -> DetailContract {
    DetailContract {
        values: &[],
        length: (1, max),
        syntax: &[],
        items: None,
    }
}

/// The contract of a member targeting an id of at most `max` characters
/// ([`ENTITY_ID_PATTERN`]).
const fn id(max: usize) -> DetailContract {
    DetailContract {
        values: &[],
        length: (1, max),
        syntax: &[DetailSyntax::Id],
        items: None,
    }
}

/// The contract of a member that is a LIST of at most `max_items` values, each
/// one `of`.
const fn list(of: DetailContract, max_items: usize) -> DetailContract {
    DetailContract {
        values: of.values,
        length: of.length,
        syntax: of.syntax,
        items: Some(max_items),
    }
}

/// The contract of a member targeting `DateTimeISO8601`: `@length(20, 20)` and
/// that shape's pattern.
const INSTANT: DetailContract = DetailContract {
    values: &[],
    length: (20, 20),
    syntax: &[DetailSyntax::Instant],
    items: None,
};

/// The contract of a member targeting an account id: `@length(12, 12)` and
/// `^\d{12}$`.
const ACCOUNT_ID: DetailContract = DetailContract {
    values: &[],
    length: (ACCOUNT_ID_DIGITS, ACCOUNT_ID_DIGITS),
    syntax: &[DetailSyntax::Digits(ACCOUNT_ID_DIGITS)],
    items: None,
};

/// The `@length(12, 12)` of every account-id member, which is also the count
/// its `^\d{12}$` states.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The contract of `OfferSummary$OfferSetId`: `OfferSetIdString`'s
/// `@length(min: 1, max: 50)` and its prefixed-id pattern.
const OFFER_SET_ID: DetailContract = DetailContract {
    values: &[],
    length: (1, 50),
    syntax: &[DetailSyntax::PrefixedId("offerset-")],
    items: None,
};

/// The contract of `OfferSummary$SubsidiaryAuthorizationId`:
/// `OfferSubsidiaryAuthorizationIdString`'s `@length(min: 1, max: 22)` and its
/// own pattern, which its doc states in words as well ("the ID begins with
/// `subauthz-` followed by 13 alphanumeric characters").
const SUBSIDIARY_AUTHORIZATION_ID: DetailContract = DetailContract {
    values: &[],
    length: (1, 22),
    syntax: &[DetailSyntax::PrefixedCode("subauthz-", 13)],
    items: None,
};

/// `OfferSummary$State`'s and `OfferSetSummary$State`'s published values:
/// `OfferStateString` and `OfferSetStateString` enumerate the same two.
pub const OFFER_STATE_VALUES: &[&str] = &["Draft", "Released"];

/// `OfferSummary$Targeting`'s published values (`OfferTargetingString`).
pub const OFFER_TARGETING_VALUES: &[&str] = &[
    "BuyerAccounts",
    "CountryCodes",
    "None",
    "ParticipatingPrograms",
];

/// `OfferSummary$TargetAgreementIntent`'s published values
/// (`OfferTargetAgreementIntentString`).
pub const OFFER_TARGET_AGREEMENT_INTENT_VALUES: &[&str] = &["Renew"];

/// `OfferSummary$CreatedBySource`'s published values
/// (`OfferCreatedBySourceString`).
pub const OFFER_CREATED_BY_SOURCE_VALUES: &[&str] = &["AwsMarketplace", "Seller"];

/// `ResaleAuthorizationSummary$Status`'s published values
/// (`ResaleAuthorizationStatusString`).
pub const RESALE_AUTHORIZATION_STATUS_VALUES: &[&str] = &["Active", "Draft", "Restricted"];

/// `ResaleAuthorizationSummary$ResellerRole`'s published values
/// (`ResaleAuthorizationResellerRoleString`).
pub const RESALE_AUTHORIZATION_RESELLER_ROLE_VALUES: &[&str] = &["ChannelPartner", "Distributor"];

/// `ExpressPrivateOfferConfigurationSummary$State`'s published values
/// (`ExpressPrivateOfferConfigurationStateString`).
pub const EXPRESS_PRIVATE_OFFER_CONFIGURATION_STATE_VALUES: &[&str] = &["ACTIVE", "INACTIVE"];

/// The members `OfferSummary` reports besides the entity's `Name`, each with
/// the contract its own target declares
/// (`smithy_query(query="shape", shape="OfferSummary")`).
const OFFER_SUMMARY_MEMBERS: &[(&str, DetailContract)] = &[
    ("ProductId", text(255)),
    ("ResaleAuthorizationId", id(255)),
    ("SubsidiaryAuthorizationId", SUBSIDIARY_AUTHORIZATION_ID),
    ("ReleaseDate", INSTANT),
    ("AvailabilityEndDate", INSTANT),
    ("BuyerAccounts", list(ACCOUNT_ID, 26)),
    ("State", choice(OFFER_STATE_VALUES)),
    ("Targeting", list(choice(OFFER_TARGETING_VALUES), 4)),
    ("OfferSetId", OFFER_SET_ID),
    ("TargetAgreementId", text(64)),
    (
        "TargetAgreementIntent",
        choice(OFFER_TARGET_AGREEMENT_INTENT_VALUES),
    ),
    ("CreatedBySource", choice(OFFER_CREATED_BY_SOURCE_VALUES)),
];

/// The same for `OfferSetSummary`
/// (`smithy_query(query="shape", shape="OfferSetSummary")`).
const OFFER_SET_SUMMARY_MEMBERS: &[(&str, DetailContract)] = &[
    ("State", choice(OFFER_STATE_VALUES)),
    ("ReleaseDate", INSTANT),
    ("AssociatedOfferIds", list(id(255), 10)),
    ("SolutionId", id(50)),
];

/// The same for `ResaleAuthorizationSummary`
/// (`smithy_query(query="shape", shape="ResaleAuthorizationSummary")`).
const RESALE_AUTHORIZATION_SUMMARY_MEMBERS: &[(&str, DetailContract)] = &[
    ("ProductId", text(255)),
    ("ProductName", text(255)),
    ("ManufacturerAccountId", ACCOUNT_ID),
    ("ManufacturerLegalName", text(255)),
    ("ResellerAccountID", ACCOUNT_ID),
    ("ResellerLegalName", text(255)),
    ("Status", choice(RESALE_AUTHORIZATION_STATUS_VALUES)),
    ("OfferExtendedStatus", text(255)),
    ("CreatedDate", INSTANT),
    ("AvailabilityEndDate", INSTANT),
    (
        "ResellerRole",
        choice(RESALE_AUTHORIZATION_RESELLER_ROLE_VALUES),
    ),
    ("SourceAuthorization", text(255)),
    ("IssuerAccountId", ACCOUNT_ID),
];

/// The same for `ExpressPrivateOfferConfigurationSummary`
/// (`smithy_query(query="shape",
/// shape="ExpressPrivateOfferConfigurationSummary")`).
const EXPRESS_PRIVATE_OFFER_CONFIGURATION_SUMMARY_MEMBERS: &[(&str, DetailContract)] = &[
    ("ProductId", id(255)),
    (
        "State",
        choice(EXPRESS_PRIVATE_OFFER_CONFIGURATION_STATE_VALUES),
    ),
];

/// `SubsidiaryAuthorizationSummary$Status`'s published values: the three its
/// own doc names ("Valid values are: `Draft` … `Active` … `Restricted`"),
/// which are the values of `SubsidiaryAuthorizationStatusString`, the enum
/// `SubsidiaryAuthorizationFilters$Status` selects on them through.
pub const SUBSIDIARY_AUTHORIZATION_STATUS_VALUES: &[&str] = &["Active", "Draft", "Restricted"];

/// The same for `SubsidiaryAuthorizationSummary`
/// (`smithy_query(query="shape", shape="SubsidiaryAuthorizationSummary")`).
///
/// Three of its members target a string shape the model bounds by LENGTH alone,
/// so [`bounded`] is their whole contract: the model declares no `@pattern` for
/// them, and this service adds none of its own.
const SUBSIDIARY_AUTHORIZATION_SUMMARY_MEMBERS: &[(&str, DetailContract)] = &[
    ("Status", choice(SUBSIDIARY_AUTHORIZATION_STATUS_VALUES)),
    ("ResourceId", bounded(255)),
    ("ResourceName", bounded(255)),
    ("SubsidiaryAccountId", ACCOUNT_ID),
    ("CreatedDate", INSTANT),
];

/// The details members the per-type summary of `entity_type` reports, each with
/// the contract the model declares for the summary member that reports it.
///
/// The five `*ProductSummary` shapes report only the entity's name and its
/// visibility, which [`entity_name_contract`] and
/// [`entity_visibility_contract`] bound per type, so a product type names
/// nothing further here — and neither does a type whose summary the model does
/// not declare ([`SHAPED_ENTITY_TYPES`]).
fn summary_member_contracts(entity_type: &str) -> &'static [(&'static str, DetailContract)] {
    match entity_type {
        "Offer" => OFFER_SUMMARY_MEMBERS,
        "OfferSet" => OFFER_SET_SUMMARY_MEMBERS,
        "ResaleAuthorization" => RESALE_AUTHORIZATION_SUMMARY_MEMBERS,
        "ExpressPrivateOfferConfiguration" => {
            EXPRESS_PRIVATE_OFFER_CONFIGURATION_SUMMARY_MEMBERS
        }
        "SubsidiaryAuthorization" => SUBSIDIARY_AUTHORIZATION_SUMMARY_MEMBERS,
        _ => &[],
    }
}

/// The contract the model declares for the member `member` of the per-type
/// summary of an entity of `entity_type`, and `None` when that summary reports
/// no member of that name.
fn summary_member_contract(entity_type: &str, member: &str) -> Option<DetailContract> {
    summary_member_contracts(entity_type)
        .iter()
        .find(|(declared, _)| *declared == member)
        .map(|(_, contract)| *contract)
}

/// The `@length(max)` of `EntityNameString`, the shape `EntitySummary$Name`
/// targets for an entity of any type.
const ENTITY_NAME_MAX: usize = 255;

/// The `@length(max)` of `OfferNameString` and `OfferSetNameString`, the two
/// per-type name members the model bounds more tightly than
/// `EntityNameString` does.
const OFFER_NAME_MAX: usize = 150;

/// The entity types whose own summary names the entity's name, as
/// `ProductTitle` or as `Name` — the types whose name satisfies a per-type
/// `@pattern("^(.)+$")` beside `EntityNameString`'s own.
///
/// Read off `EntitySummary`'s per-type summary members: the five product
/// summaries name `ProductTitle` ([`PRODUCT_ENTITY_TYPES`]) and three more name
/// `Name`. `ExpressPrivateOfferConfigurationSummary` names neither, so an
/// entity of that type reports its name only through the common member — and so
/// does an entity whose per-type name member declares no pattern beside
/// `EntityNameString`'s (`SubsidiaryAuthorizationSummary$Name` targets a shape
/// bounded by `@length` alone), since there is no further syntax to satisfy.
const NAMED_ENTITY_TYPES: &[&str] = &["Offer", "OfferSet", "ResaleAuthorization"];

/// The contract the value an entity's `Name` is read from
/// ([`ENTITY_NAME_DETAIL_MEMBERS`]) has to satisfy for an entity of
/// `entity_type`.
///
/// One value is reported by two members: `EntitySummary$Name` reports it for
/// every type (`EntityNameString`: one to [`ENTITY_NAME_MAX`] characters whose
/// first is not whitespace), and the summary of a type that names it reports
/// the same value under its own member — so the value has to satisfy BOTH, and
/// this contract is their intersection. `OfferNameString` and
/// `OfferSetNameString` bound it at [`OFFER_NAME_MAX`]; every other per-type
/// name member (`AmiProductTitleString` and the four product titles beside it,
/// `ResaleAuthorizationNameString`) carries the same max as
/// `EntityNameString`.
pub fn entity_name_contract(entity_type: &str) -> DetailContract {
    /// `EntityNameString`'s pattern with the per-type member's beside it.
    const NAMED: &[DetailSyntax] = &[DetailSyntax::OneLine, DetailSyntax::NonBlankStart];
    /// `EntityNameString`'s pattern alone.
    const COMMON: &[DetailSyntax] = &[DetailSyntax::NonBlankStart];

    let max = match entity_type {
        "Offer" | "OfferSet" => OFFER_NAME_MAX,
        _ => ENTITY_NAME_MAX,
    };
    let names_its_own = PRODUCT_ENTITY_TYPES.contains(&entity_type)
        || NAMED_ENTITY_TYPES.contains(&entity_type);
    DetailContract {
        values: &[],
        length: (1, max),
        syntax: if names_its_own { NAMED } else { COMMON },
        items: None,
    }
}

/// The `ErrorDetail`s applying a change to an entity of `entity_type` finds
/// among the details members the catalog READS.
///
/// The members of a change's details belong to its change type, which the model
/// declares no shape for, so what the catalog can judge is what it reads — and
/// it reads a member to REPORT it, so the contract each one is judged against is
/// the one the model declares for the `EntitySummary` member that reports that
/// value: the published set of an enumerated member, and the `@length` and
/// `@pattern` of a member targeting a string shape. A member an entity's `Name`
/// is read from ([`ENTITY_NAME_DETAIL_MEMBERS`] — the one this type names it
/// under, [`entity_name_detail_member`], and the one another type does, which a
/// change may carry just as well) is bounded by [`entity_name_contract`], the
/// member its `Visibility` comes from ([`ENTITY_VISIBILITY_DETAIL_MEMBER`]) by
/// the values the `*VisibilityString` enum of the entity's own type publishes
/// ([`entity_visibility_contract`]), and every member the listed entity's
/// per-type summary reports by
/// [`summary_member_contracts`]. A member NO summary reports is carried into
/// the entity's details untouched and reported only inside the opaque `Details`
/// document, so nothing here judges it.
pub fn details_defects(entity_type: &str, details: Option<&Value>) -> Vec<ErrorDetail> {
    let mut defects: Vec<ErrorDetail> = Vec::new();
    let Some(Value::Object(members)) = details else {
        return defects;
    };
    let name = entity_name_contract(entity_type);
    for member in ENTITY_NAME_DETAIL_MEMBERS {
        if let Some(value) = members.get(*member) {
            if !name.admits(value) {
                defects.push(invalid_detail_value(member, &name.accepts(), value));
            }
        }
    }
    let visibility = entity_visibility_contract(entity_type);
    if let Some(value) = members.get(ENTITY_VISIBILITY_DETAIL_MEMBER) {
        if !visibility.admits(value) {
            defects.push(invalid_detail_value(
                ENTITY_VISIBILITY_DETAIL_MEMBER,
                &visibility.accepts(),
                value,
            ));
        }
    }
    for (member, contract) in summary_member_contracts(entity_type) {
        if ENTITY_NAME_DETAIL_MEMBERS.contains(member) {
            continue;
        }
        if let Some(value) = members.get(*member) {
            if !contract.admits(value) {
                defects.push(invalid_detail_value(member, &contract.accepts(), value));
            }
        }
    }
    defects
}

/// The `ErrorDetail` for a details member whose value the catalog cannot read,
/// built from the change: the member, what that member accepts, and the value
/// the change gave it.
fn invalid_detail_value(member: &str, accepts: &str, value: &Value) -> ErrorDetail {
    ErrorDetail {
        error_code: Some(ERROR_CODE_INVALID_VALUE.to_string()),
        error_message: Some(format!(
            "Details.{member} accepts {accepts}, not {}",
            quoted(value)
        )),
    }
}

/// The `FailureDescription` of a change set that contradicts itself: two of its
/// changes claim a different value for the SAME attribute of the same entity,
/// so each change is applicable on its own and it is their combination the
/// catalog cannot apply.
///
/// Only the members the catalog reads into one of the entity's own FIELDS — the
/// members an entity's name is read from ([`ENTITY_NAME_DETAIL_MEMBERS`], the
/// union over the types, since one change set carries changes of several types)
/// and its visibility — are claims about the entity: every other member is
/// carried into the entity's details, where the last change to name it is the
/// one that stands, so two changes naming it is a merge rather than a
/// contradiction. Each change is therefore looked at through that fixed, short
/// list of members, never by walking the submitted document, and
/// `RequestedChangeList` bounds the changes at 20 — so the work here is bounded
/// by the request's modelled length and not by how wide a details document the
/// caller sent.
fn contradicting_details(changes: &[ChangeRecord]) -> Option<String> {
    let read_members = || {
        ENTITY_NAME_DETAIL_MEMBERS
            .iter()
            .chain(::std::iter::once(&ENTITY_VISIBILITY_DETAIL_MEMBER))
    };
    let mut claimed: HashMap<(String, &str), Value> = HashMap::new();
    for change in changes {
        let entity_id = change.entity_id.clone();
        let Some(Value::Object(members)) = change.details_value() else {
            continue;
        };
        for member in read_members() {
            let Some(value) = members.get(*member) else {
                continue;
            };
            let key = (entity_id.clone(), *member);
            match claimed.get(&key) {
                Some(held) if held != value => {
                    return Some(format!(
                        "The changes of this request give {member} of entity {entity_id} both \
                         {} and {}.",
                        quoted(held),
                        quoted(value)
                    ))
                }
                Some(_) => continue,
                None => {
                    claimed.insert(key, value.clone());
                }
            }
        }
    }
    None
}

/// A submitted details value as a message quotes it: its JSON rendering as a
/// single line ([`one_line`]), cut at [`DETAIL_VALUE_QUOTE_LIMIT`] characters so
/// the message built around it stays inside `ExceptionMessageContent`'s 2048.
///
/// JSON escapes a line feed and a carriage return itself; the remaining line
/// terminators it renders verbatim, so the rendering goes through [`one_line`]
/// for the same reason a sentence's value does.
fn quoted(value: &Value) -> String {
    cut_to(
        &one_line(&value.to_string()),
        DETAIL_VALUE_QUOTE_LIMIT,
        CUT_MARK,
    )
}

/// `text` cut to `limit` characters, `mark` appended where it was cut and
/// nothing appended where it was not — the one place this crate shortens a value
/// it quotes back, so every bound a message or a reported value is held to is
/// enforced the same way ([`scope_text`], [`quoted`], [`quoting`]).
fn cut_to(text: &str, limit: usize, mark: &str) -> String {
    match text.char_indices().nth(limit) {
        Some((cut, _)) => format!("{}{mark}", &text[..cut]),
        None => text.to_string(),
    }
}

/// True when `value` is a `DateTimeISO8601`: the 20-character
/// `....-..-..T..:..:..Z` form that shape's pattern pins, which is what makes a
/// byte comparison of two of them a comparison of two instants.
///
/// The boundary validator refuses a modelled `DateTimeISO8601` member, so this
/// is for a value the model does not constrain — a `Filter$ValueList` entry a
/// before/after filter compares against.
pub fn is_iso8601(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let digits = |range: ::std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    let number = |range: ::std::ops::Range<usize>| value[range].parse::<u32>().unwrap_or(u32::MAX);
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
        && digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && digits(11..13)
        && digits(14..16)
        && digits(17..19)
        && (1..=12).contains(&number(5..7))
        && (1..=31).contains(&number(8..10))
        && number(11..13) <= 23
        && number(14..16) <= 59
        && number(17..19) <= 59
}

/// `at` in the `DateTimeISO8601` form every timestamp of this service is
/// modelled with: exactly 20 characters, `....-..-..T..:..:..Z`.
pub fn iso8601(at: SystemTime) -> String {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let text = ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `Format::DateTime` renders a fractional part only for a non-zero
    // subsecond and `from_secs` carries none, so this keeps the 20-character
    // form the pattern requires even if that ever changes.
    match text.split_once('.') {
        Some((whole, _)) => format!("{whole}Z"),
        None => text,
    }
}

/// The page `offset` as a `NextToken`: `^[\w+=.:@\-\/]+$`, with a prefix naming
/// the list it pages, so a token one list returned is not accepted by another.
pub fn encode_page_token(prefix: &str, offset: usize) -> String {
    format!("{prefix}-{offset:x}")
}

/// The offset a `NextToken` this service issued for `prefix` resumes at, or
/// `None` for a token it never issued.
///
/// Reading the digits is not enough to accept a token: `usize::from_str_radix`
/// also reads spellings [`encode_page_token`] never writes — a leading `+`,
/// leading zeros, uppercase hexadecimal digits — and the modelled `NextToken`
/// pattern admits all of them, so they would reach here. A token names a page
/// only when it is, character for character, the token this service mints for
/// the offset it carries; every other spelling names no page, exactly like a
/// non-hexadecimal one.
pub fn decode_page_token(prefix: &str, token: &str) -> Option<usize> {
    let rest = token.strip_prefix(prefix)?.strip_prefix('-')?;
    let offset = usize::from_str_radix(rest, 16).ok()?;
    (encode_page_token(prefix, offset) == token).then_some(offset)
}

/// How long a details value this service stores and reports may be.
///
/// `Json` — the shape of every member that carries a details value as a string,
/// on the way in (`Change$Details`) and on the way out
/// (`DescribeEntityResponse$Details`, `ChangeSummary$Details`) — is
/// `@length(min: 2, max: 16384)`
/// (`smithy_query(query="shape", shape="Json")`). `JsonDocumentType` carries the
/// SAME details as a document and bounds nothing, so this is the bound a details
/// value is judged against whichever member carried it: a value this service
/// accepted and accumulated has to stay reportable as that string.
pub const DETAILS_JSON_LIMIT: usize = 16_384;

/// How many characters the `Json` string a details value is reported as takes —
/// [`EntityRecord::details_json`]'s own serialization, which writes no
/// insignificant whitespace, so the count is the length of the string a reader
/// receives.
///
/// Counted in CHARACTERS, which is the unit a modelled `@length` on a string
/// states — the same unit the boundary walk holds `Change$Details` to — so a
/// value the boundary admitted against [`DETAILS_JSON_LIMIT`] is measured here
/// against that same bound, and not against its UTF-8 byte count, which is
/// larger for every value that is not pure ASCII.
///
/// A map of JSON values has no serialization failure to report, and the
/// unreachable case counts as PAST the bound rather than inside it: a length
/// nothing could measure is not one this service may report.
pub fn details_json_length(members: &Map<String, Value>) -> usize {
    serde_json::to_string(members).map_or(usize::MAX, |text| text.chars().count())
}

/// How many characters an id the catalog assigns adds to one delivery option:
/// the quoted member name, the colon and the quoted id — every minted id is
/// [`DELIVERY_OPTION_ID_PREFIX`] and twelve hex digits
/// ([`ComAmazonawsMarketplacecatalogAwsmpseymourState::mint_delivery_option_id`])
/// — plus the comma separating it from the members the option already carried.
/// An option that carries nothing else needs no comma, so counting it is
/// conservative by that one character rather than short by it.
const MINTED_DELIVERY_OPTION_ID_LENGTH: usize =
    DELIVERY_OPTION_ID_MEMBER.len() + DELIVERY_OPTION_ID_PREFIX.len() + 12 + 6;

/// How much longer than `details` the value applying a change STORES will be: a
/// delivery option that carries no id is given one before the entity is
/// written, which is a member the document does not carry yet. Minted ids are
/// fixed-width, so this growth is known before the change runs — which is what
/// lets the merge be judged against [`DETAILS_JSON_LIMIT`] while the change set
/// is still only a plan.
///
/// Measured on the MERGED details rather than on one change's own, because a
/// change carrying [`DELIVERY_OPTIONS_DETAIL_MEMBER`] replaces the list the
/// merge held: what still needs ids is whatever the merge carries at that
/// point, and an option already holding an id grows by nothing.
pub fn applied_details_growth(details: &Map<String, Value>) -> usize {
    let Some(options) = details
        .get(DELIVERY_OPTIONS_DETAIL_MEMBER)
        .and_then(Value::as_array)
    else {
        return 0;
    };
    options
        .iter()
        .take(MAX_DELIVERY_OPTIONS_PER_CHANGE)
        .filter(|option| {
            option.is_object()
                && option
                    .get(DELIVERY_OPTION_ID_MEMBER)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
        })
        .count()
        * MINTED_DELIVERY_OPTION_ID_LENGTH
}

/// The modelled `ValidationException` payload for a change whose details are
/// longer than [`DETAILS_JSON_LIMIT`] — built from the request: the member that
/// carried them, the length it sent and the bound it passed.
pub fn details_too_long(field: &str, length: usize) -> ValidationException {
    validation(
        field,
        format!(
            "the details of a change are at most {DETAILS_JSON_LIMIT} characters of JSON, \
             not {length}"
        ),
    )
}

/// The deepest `Details` / `DetailsDocument` this service accepts.
///
/// Neither `Json` nor `JsonDocumentType` bounds nesting, but converting, storing
/// and reporting a details value walks it once per level, so a value nested
/// without limit would exhaust the stack — which Rust cannot catch. 64 levels is
/// far deeper than any entity description and below `serde_json`'s own
/// 128-level recursion guard, so a value this service accepts can always be
/// parsed, converted and read back.
pub const MAX_DETAILS_DEPTH: usize = 64;

/// True when `value` nests deeper than [`MAX_DETAILS_DEPTH`].
///
/// Counted with an explicit stack rather than by recursing, so measuring an
/// arbitrarily deep value is itself bounded.
pub fn exceeds_details_depth(value: &Value) -> bool {
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_DETAILS_DEPTH {
            return true;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, depth + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// `serde_json`'s value as the `Document` the model's `JsonDocumentType`
/// members are deserialized into, or `None` when it nests deeper than
/// [`MAX_DETAILS_DEPTH`].
pub fn json_to_document(value: &Value) -> Option<Document> {
    to_document(value, MAX_DETAILS_DEPTH)
}

/// [`json_to_document`] with the levels it may still descend, so the recursion
/// can never outrun [`MAX_DETAILS_DEPTH`] frames.
fn to_document(value: &Value, budget: usize) -> Option<Document> {
    let next = budget.checked_sub(1)?;
    Some(match value {
        Value::Null => Document::Null,
        Value::Bool(flag) => Document::Bool(*flag),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                Document::Number(::aws_smithy_types::Number::PosInt(unsigned))
            } else if let Some(signed) = number.as_i64() {
                Document::Number(::aws_smithy_types::Number::NegInt(signed))
            } else {
                Document::Number(::aws_smithy_types::Number::Float(
                    number.as_f64().unwrap_or_default(),
                ))
            }
        }
        Value::String(text) => Document::String(text.clone()),
        Value::Array(items) => Document::Array(
            items
                .iter()
                .map(|item| to_document(item, next))
                .collect::<Option<Vec<Document>>>()?,
        ),
        Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(key, value)| to_document(value, next).map(|value| (key.clone(), value)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// The inverse of [`json_to_document`], so a submitted `DetailsDocument` is
/// stored, merged and compared as one JSON value whichever member carried it.
/// `None` for a document nested deeper than [`MAX_DETAILS_DEPTH`], which the
/// handler refuses rather than storing.
pub fn document_to_json(document: &Document) -> Option<Value> {
    to_json(document, MAX_DETAILS_DEPTH)
}

/// [`document_to_json`] with the levels it may still descend.
fn to_json(document: &Document, budget: usize) -> Option<Value> {
    let next = budget.checked_sub(1)?;
    Some(match document {
        Document::Null => Value::Null,
        Document::Bool(flag) => Value::Bool(*flag),
        Document::Number(number) => match number {
            ::aws_smithy_types::Number::PosInt(unsigned) => Value::from(*unsigned),
            ::aws_smithy_types::Number::NegInt(signed) => Value::from(*signed),
            ::aws_smithy_types::Number::Float(float) => {
                serde_json::Number::from_f64(*float).map_or(Value::Null, Value::Number)
            }
        },
        Document::String(text) => Value::String(text.clone()),
        Document::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| to_json(item, next))
                .collect::<Option<Vec<Value>>>()?,
        ),
        Document::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, value)| to_json(value, next).map(|value| (key.clone(), value)))
                .collect::<Option<_>>()?,
        ),
    })
}

/// The modelled `ValidationException` payload stated the way this service states
/// one: everything it has to say about the request is in `Message`, as the
/// bracketed list of per-member sentences, and `ValidationExceptionFieldList` is
/// left absent.
///
/// The form is the service's own, read off its recorded refusals: two different
/// operations answer a rejected member with a `Message` of exactly that shape
/// (`[Requested entity id '…' is invalid. It should match with …]`,
/// `[Requested sort by '…' is invalid. It should be one of '…'.]`) and with no
/// `ValidationExceptionFieldList` beside it. So the message is what names the
/// member, and every refusal built here passes one that does.
pub fn validation_message(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(format!("[{}]", message.into())),
        validation_exception_field_list: None,
    }
}

/// A message built around a submitted value: `prefix`, the value quoted as a
/// single line ([`one_line`]), then `suffix` — with the quote cut to the room
/// those two leave inside [`MESSAGE_BUDGET`], marked [`CUT_MARK`] where it was
/// cut ([`cut_to`]).
///
/// The same rule [`quoted`] follows for a submitted details value and
/// [`scope_text`] for a submitted scope value, for the values a REFUSAL quotes:
/// a member this service judges with a predicate of its own is not bounded by
/// the boundary walk, and a member that IS can be as long on its own as a
/// message may be, so how much of it a sentence can quote is decided here rather
/// than by what was sent. The sentence itself is never cut — a caller reads the
/// whole fault, and the value as far as it fits.
///
/// The value is rendered single-line BEFORE it is cut, so the budget counts the
/// characters the message actually carries.
fn quoting(prefix: &str, value: &str, suffix: &str) -> String {
    let room = MESSAGE_BUDGET.saturating_sub(
        prefix.chars().count() + suffix.chars().count() + CUT_MARK.chars().count(),
    );
    format!("{prefix}{}{suffix}", cut_to(&one_line(value), room, CUT_MARK))
}

/// The modelled `ValidationException` payload for a request this service
/// refuses, naming the member that was wrong — [`validation_message`] with the
/// member named ahead of what was wrong with it.
pub fn validation(field: &str, message: impl Into<String>) -> ValidationException {
    validation_message(format!("{field}: {}", message.into()))
}

/// The modelled `ValidationException` payload for a tag key no caller may name:
/// one inside the namespace [`RESERVED_TAG_KEY_PREFIX`] keeps.
///
/// Stated against `field` — the request member that carried the key — and built
/// from the request, so the refusal names the offending key the caller sent.
///
/// The key is quoted through [`quoting`], like every other sentence this service
/// builds around a submitted value: a tag key's own `@pattern`
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) admits the separators of the Unicode `Z`
/// category, line separator and paragraph separator among them, which the member
/// carrying this sentence admits none of.
pub fn reserved_tag_key(field: &str, key: &str) -> ValidationException {
    validation(
        field,
        quoting(
            "tag key '",
            key,
            &format!(
                "' begins with '{RESERVED_TAG_KEY_PREFIX}', the prefix reserved for \
                 the tags Amazon Web Services creates, which a caller cannot create, edit \
                 or remove"
            ),
        ),
    )
}

/// The modelled `ValidationException` payload for a member this service could
/// not read at all.
///
/// Stated like any other refusal: the only thing that distinguished it was
/// `ValidationExceptionField$Reason`, and this service reports no field list to
/// carry a reason in, so the message says what could not be read.
pub fn cannot_parse(field: &str, message: impl Into<String>) -> ValidationException {
    validation(field, message)
}

/// The modelled `ValidationException` payload for a `Policy` that is not a
/// resource-based policy document.
///
/// `ResourcePolicyJson` bounds only the length and the characters of the text, so
/// what makes it a POLICY is the IAM JSON policy grammar
/// ([`service_common::iam_policy::resource_policy_defect`]), and a document
/// outside it is refused rather than stored as a policy no reader could evaluate.
/// The refusal is built from the request: it names the member and the clause of
/// that grammar the submitted document broke.
pub fn invalid_resource_policy(
    field: &str,
    defect: ::service_common::iam_policy::PolicyDefect,
) -> ValidationException {
    validation(
        field,
        format!(
            "{field} must be a resource-based policy document formatted in JSON: {defect}"
        ),
    )
}

/// The modelled `ValidationException` payload for a change set that contradicts
/// itself: two of its changes carry the same change type against the same
/// entity.
///
/// `StartChangeSet` documents the rule ("Within a single `ChangeSet`, you can't
/// start the same change type against the same entity multiple times"), and it
/// is a rule about the REQUEST — so it is judged on the entity the request names,
/// before that entity is resolved, and the fault is built from the request: the
/// change type that was repeated and the entity it was repeated on.
pub fn duplicate_change_pair(change_type: &str, entity_id: &str) -> ValidationException {
    validation_message(format!(
        "Requested changes contain multiple changes for the same change type \
         '{change_type}' on entity '{entity_id}'. Entity - change type pairs \
         should be unique."
    ))
}

/// The modelled `ValidationException` payload for a `Catalog` other than the
/// fixed value every operation documents.
///
/// The value has passed [`is_catalog_name`] to get here, so it is inside the
/// length a catalog name may be and the sentence quotes it whole.
pub fn wrong_catalog(catalog: &str) -> ValidationException {
    validation(
        "Catalog",
        format!("Catalog must be {AWS_MARKETPLACE_CATALOG}, not {catalog:?}"),
    )
}

/// The syntax the service documents for a catalog NAME, which it reports in the
/// fault for a `Catalog` outside it.
///
/// It is WIDER than the modelled `Catalog` `@pattern` (`^[a-zA-Z]+$`) — digits,
/// dots and hyphens are admitted — and narrower in its length (47, not 64), so
/// the model's pattern cannot be what a caller is told to satisfy. This is the
/// pattern the service's own recorded refusals quote, and
/// [`is_catalog_name`] is this string spelled out as a predicate.
pub const CATALOG_PATTERN: &str =
    r"^(?![.-])(?!.*[.-]{2})(?![0-9]+\.[0-9.]+$)[a-zA-Z0-9.-]{1,47}(?<![.-])$";

/// How long a catalog name may be: the `{1,47}` of [`CATALOG_PATTERN`].
const CATALOG_NAME_MAX: usize = 47;

/// Whether `value` is a catalog NAME — [`CATALOG_PATTERN`] as a predicate: one
/// to [`CATALOG_NAME_MAX`] characters of `[a-zA-Z0-9.-]`, neither starting nor
/// ending with `.` or `-`, with no two of those adjacent, and not a bare version
/// number (`[0-9]+\.[0-9.]+`).
pub fn is_catalog_name(value: &str) -> bool {
    if value.is_empty() || value.chars().count() > CATALOG_NAME_MAX {
        return false;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return false;
    }
    let separator = |c: char| matches!(c, '.' | '-');
    if value.starts_with(separator) || value.ends_with(separator) {
        return false;
    }
    if value
        .as_bytes()
        .windows(2)
        .any(|pair| pair.iter().all(|byte| separator(char::from(*byte))))
    {
        return false;
    }
    !is_version_number(value)
}

/// Whether `value` is the `[0-9]+\.[0-9.]+` form [`CATALOG_PATTERN`] excludes:
/// digits, a dot, then digits and dots only.
fn is_version_number(value: &str) -> bool {
    let Some((head, tail)) = value.split_once('.') else {
        return false;
    };
    !head.is_empty()
        && head.bytes().all(|byte| byte.is_ascii_digit())
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// The modelled `ValidationException` payload for a `Catalog` outside
/// [`CATALOG_PATTERN`] — built from the request, naming the value it rejected and
/// the syntax a catalog name has to satisfy.
pub fn invalid_catalog(catalog: &str) -> ValidationException {
    validation_message(quoting(
        "Requested catalog '",
        catalog,
        &format!("' is invalid. It should match with {CATALOG_PATTERN}."),
    ))
}

/// The `Catalog` check every operation of this service runs before it does
/// anything else: a value that is not a catalog NAME at all is refused for its
/// syntax, and a well-formed name other than the one catalog this service serves
/// is refused as naming nothing it has.
///
/// The two are different faults: the first says the request is malformed, the
/// second that the catalog it named is not this one.
pub fn check_catalog(catalog: &str) -> Result<(), ValidationException> {
    if !is_catalog_name(catalog) {
        return Err(invalid_catalog(catalog));
    }
    if catalog != AWS_MARKETPLACE_CATALOG {
        return Err(wrong_catalog(catalog));
    }
    Ok(())
}

/// Whether `value` names one of [`ENTITY_TYPES`].
pub fn is_entity_type(value: &str) -> bool {
    ENTITY_TYPES.contains(&value)
}

/// The modelled `ValidationException` payload for an `EntityType` this catalog
/// has no such type for.
///
/// The fault names the value and NOT a pattern: the value may well satisfy
/// `EntityType`'s `@pattern` and still name no type, so there is no syntax to
/// tell the caller to satisfy — the set in [`ENTITY_TYPES`] is what it failed.
pub fn invalid_entity_type(entity_type: &str) -> ValidationException {
    validation_message(quoting(
        "Requested entity type '",
        entity_type,
        "' is invalid.",
    ))
}

/// Whether `value` names one of the change types this catalog performs
/// ([`CHANGE_TYPES`]).
pub fn is_change_type(value: &str) -> bool {
    CHANGE_TYPES.contains(&value)
}

/// The modelled `ValidationException` payload for a `Change$ChangeType` this
/// catalog performs no such change for.
///
/// Stated like [`invalid_entity_type`], and for the same reason: the value may
/// well satisfy `ChangeType`'s `@pattern` and still name no change, so there is
/// no syntax to tell the caller to satisfy — the set in [`CHANGE_TYPES`] is what
/// it failed.
///
/// The member is judged here rather than at the boundary (`skip_validation` on
/// `Change`), so the value reaching this sentence is bounded by nothing the
/// walk checked: it is quoted through [`quoting`], which keeps the fault inside
/// the length `ExceptionMessageContent` declares however long a value was sent.
pub fn invalid_change_type(change_type: &str) -> ValidationException {
    validation_message(quoting(
        "Requested change type '",
        change_type,
        "' is invalid.",
    ))
}

/// The entity type a `Change$Entity$Type` names: everything before the optional
/// `@<version>` suffix.
///
/// `Entity$Type` addresses an entity type at a SCHEMA VERSION
/// (`DataProduct@1.0`) — the form every real caller sends and the form the
/// service accepts — while the entity itself is of the type the reference names.
pub fn entity_type_of(reference: &str) -> &str {
    reference.split('@').next().unwrap_or(reference)
}

/// The schema version a `Change$Entity$Type` addresses its type at: the
/// `@<version>` part of the reference, and `None` for a reference carrying
/// none.
///
/// The counterpart of [`entity_type_of`]: between them they split the reference
/// into the type the entity IS and the version the change addressed it through,
/// which is what the entity reads report the type at
/// ([`EntityRecord::entity_type_reference`]).
pub fn entity_type_version_of(reference: &str) -> Option<&str> {
    reference
        .split_once(ENTITY_TYPE_VERSION_SEPARATOR)
        .map(|(_, version)| version)
        .filter(|version| !version.is_empty())
}

/// How long any member of this service that names an entity, an entity type or
/// a revision of one may be: the `@length(max: …)` the model puts on every one
/// of those shapes — `EntityType`, `Identifier`, `EntityId` and the wider
/// `ResourceId` all carry `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="Entity,Identifier,EntityType,EntityId")`).
///
/// A member whose boundary validation this service replaces with a predicate of
/// its own is still bounded by it: the predicate stands in for the whole
/// constraint, length included, so no value past it is accepted, stored or
/// served back in a shape that forbids it.
pub const ENTITY_MEMBER_MAX: usize = 255;

/// The syntax the service publishes for `Change$Entity$Type`: an entity type at
/// a SCHEMA VERSION, the two parts separated by
/// [`ENTITY_TYPE_VERSION_SEPARATOR`].
///
/// `Entity$Type` targets `EntityType`, whose modelled `@pattern` (`^[a-zA-Z]+$`,
/// `smithy_query(query="shape", shape="Entity")`) admits no separator at all, so
/// the model's pattern cannot be what a caller is told to satisfy here — this is
/// the syntax the service's own recorded refusals quote for the member, and it
/// is what the member carries off the boundary (`skip_validation` on `Entity`).
pub const ENTITY_TYPE_REFERENCE_PATTERN: &str = "^(.*)@(.*)$";

/// What separates the two parts of an `Entity$Type`, and of every other
/// versioned id this service reads.
const ENTITY_TYPE_VERSION_SEPARATOR: char = '@';

/// Whether `reference` is an `Entity$Type`: one of [`ENTITY_TYPES`] at a version
/// (`<EntityType>@<version>`), within the modelled [`ENTITY_MEMBER_MAX`]
/// characters the member is bounded by.
///
/// A change addresses an entity type at a schema version, so a bare type name is
/// not one of these references — [`check_entity_type_reference`] is what refuses
/// it, and for the published FORM rather than for the set of types.
///
/// The length is judged on the whole reference, which is the value the member
/// carries: a type this catalog has at a version long enough to take the
/// reference past that bound is a reference no `EntityType` can hold, so it is
/// refused rather than stored and reported back out of contract.
pub fn is_entity_type_reference(reference: &str) -> bool {
    if !(1..=ENTITY_MEMBER_MAX).contains(&reference.chars().count()) {
        return false;
    }
    match reference.split_once(ENTITY_TYPE_VERSION_SEPARATOR) {
        Some((entity_type, version)) => is_entity_type(entity_type) && is_version_suffix(version),
        None => false,
    }
}

/// The modelled `ValidationException` payload for a `Change$Entity$Type` that is
/// not of the published `<EntityType>@<version>` form — built from the request,
/// naming the value it rejected and the syntax it had to satisfy.
pub fn unqualified_entity_type(reference: &str) -> ValidationException {
    validation_message(quoting(
        "Requested entity type '",
        reference,
        &format!("' is invalid. It should match with {ENTITY_TYPE_REFERENCE_PATTERN}."),
    ))
}

/// The `Change$Entity$Type` check `StartChangeSet` runs on every change: the
/// value is first held to the FORM the service publishes for the member
/// ([`ENTITY_TYPE_REFERENCE_PATTERN`]), and only a value of that form is then
/// held to the set of types this catalog HAS.
///
/// The two are different faults, as they are for `Catalog` ([`check_catalog`]):
/// the first says the member is not written the way the service publishes it —
/// so the sentence quotes that syntax — and the second that the type it named is
/// not one of this catalog's, which no syntax could tell a caller.
pub fn check_entity_type_reference(reference: &str) -> Result<(), ValidationException> {
    if !reference.contains(ENTITY_TYPE_VERSION_SEPARATOR) {
        return Err(unqualified_entity_type(reference));
    }
    if !is_entity_type_reference(reference) {
        return Err(invalid_entity_type(reference));
    }
    Ok(())
}

/// Whether `version` is the `@<version>` part of one of this service's versioned
/// ids: a non-empty run of alphanumerics separated by dots, which is the form the
/// model's own versioned ids take (`VersionedFrameworkId`'s doc: "frameworkId@version
/// (for example, AMISecurity@1.0)", `Entity$Type`'s `DataProduct@1.0`).
///
/// One form for every versioned id this service reads, so an entity type at a
/// schema version and a framework at a framework version are judged alike.
fn is_version_suffix(version: &str) -> bool {
    !version.is_empty()
        && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        && !version.starts_with('.')
        && !version.ends_with('.')
        && !version.contains("..")
}

/// The framework of [`ASSESSMENT_FRAMEWORKS`] `framework_id` names, or `None`
/// for a value naming no framework this catalog assesses against: one of them on
/// its own — the unversioned form `ListAssessmentsRequest$FrameworkId` documents
/// ("For example, AMISecurity") — or that framework at a version, the
/// `frameworkId@version` form `VersionedFrameworkId` states.
///
/// The member's own `@length` and `@pattern` are judged at the boundary, so what
/// is left is the one thing a published set decides: whether the value names a
/// framework this catalog HAS. One that does not names no framework an
/// assessment can have been performed against.
pub fn assessment_framework(framework_id: &str) -> Option<&'static str> {
    let framework = match framework_id.split_once('@') {
        Some((framework, version)) if is_version_suffix(version) => framework,
        Some(_) => return None,
        None => framework_id,
    };
    ASSESSMENT_FRAMEWORKS
        .iter()
        .copied()
        .find(|published| *published == framework)
}

/// Whether `framework_id` names a framework this catalog assesses against —
/// [`assessment_framework`] as a predicate.
pub fn is_assessment_framework_id(framework_id: &str) -> bool {
    assessment_framework(framework_id).is_some()
}

/// The modelled `ValidationException` payload for a `FrameworkId` naming no
/// framework this catalog assesses against.
///
/// Stated like every other closed-set refusal of this service
/// ([`invalid_choice`]), and the set it quotes is the unversioned ids
/// `FrameworkId`'s own documentation gives, which is what a caller sends.
pub fn invalid_framework_id(framework_id: &str) -> ValidationException {
    invalid_choice("framework", framework_id, ASSESSMENT_FRAMEWORKS)
}

/// The syntax the service documents for `Entity$Identifier`, which it reports in
/// the fault for one outside it.
///
/// Two alternatives: an entity id optionally at a revision, and the
/// `$<ChangeName>.Entity.Identifier` reference by which one change of a change
/// set names the entity another change of the SAME change set creates. Both are
/// wider than the modelled `Identifier` `@pattern` (`^[\w\-@]+$`) in the
/// characters they admit (`.` and `/`) and narrower in their shape, so the
/// model's pattern is not what a caller is told to satisfy.
///
/// The string states no length: it is the syntax this service's own recorded
/// refusals quote, and they quote it with the unbounded runs written here. The
/// member's modelled `@length` is a requirement all the same, so
/// [`is_entity_identifier`] enforces [`ENTITY_MEMBER_MAX`] beside this pattern
/// rather than inside it.
pub const ENTITY_IDENTIFIER_PATTERN: &str =
    r"(^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9](@\d+)?$)|(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)";

/// The suffix of the change-reference alternative of [`ENTITY_IDENTIFIER_PATTERN`].
const CHANGE_REFERENCE_SUFFIX: &str = ".Entity.Identifier";

/// Whether `value` is an `Entity$Identifier`: [`ENTITY_IDENTIFIER_PATTERN`]
/// spelled out as a predicate.
///
/// Only the change-reference alternative begins with `$` — no entity id does —
/// so that character is what decides which alternative a value is read as.
///
/// The modelled [`ENTITY_MEMBER_MAX`] bounds the whole value, whichever
/// alternative it is read as, so an identifier past it is refused here — before
/// any entity is looked up — rather than reported missing.
pub fn is_entity_identifier(value: &str) -> bool {
    if !(1..=ENTITY_MEMBER_MAX).contains(&value.chars().count()) {
        return false;
    }
    if let Some(rest) = value.strip_prefix('$') {
        return rest
            .strip_suffix(CHANGE_REFERENCE_SUFFIX)
            .is_some_and(is_change_name);
    }
    match value.split_once('@') {
        Some((id, revision)) => {
            !revision.is_empty()
                && revision.bytes().all(|byte| byte.is_ascii_digit())
                && is_entity_id(id)
        }
        None => is_entity_id(value),
    }
}

/// The entity id an `Entity$Identifier` names: the id without the `@<revision>`
/// an identifier may address it at.
pub fn entity_id_of(identifier: &str) -> &str {
    identifier.split('@').next().unwrap_or(identifier)
}

/// The modelled `ValidationException` payload for an `Entity$Identifier` outside
/// [`ENTITY_IDENTIFIER_PATTERN`] — built from the request, naming the value and
/// the syntax it had to satisfy.
pub fn invalid_entity_identifier(identifier: &str) -> ValidationException {
    validation_message(quoting(
        "Requested entity identifier '",
        identifier,
        &format!("' is invalid. It should match with {ENTITY_IDENTIFIER_PATTERN}."),
    ))
}

/// The `ChangeName` a `$<ChangeName>.Entity.Identifier` identifier names, and
/// `None` for an identifier that names an entity id instead.
///
/// This is the change-reference alternative of [`ENTITY_IDENTIFIER_PATTERN`]
/// read for the one thing inside it that carries meaning: which change of the
/// same change set the entity belongs to.
pub fn change_reference_name(identifier: &str) -> Option<&str> {
    identifier
        .strip_prefix('$')?
        .strip_suffix(CHANGE_REFERENCE_SUFFIX)
        .filter(|name| is_change_name(name))
}

/// Where a chain of change references ENDS: the change that the change at
/// `start` acts through, which is the one naming an entity of its own or
/// creating one.
///
/// `references[i]` is the change the change at `i` names through
/// [`change_reference_name`], and `None` for a change that names an entity id
/// or no entity at all. A change acts on the entity of the change it names, so
/// following the chain to its end is what says which entity that is. A chain
/// that returns to a change it has already passed names no entity, and is
/// reported by `None`: at most one step per change is taken, so the walk is
/// bounded by the change set's length however the references are written.
pub fn reference_chain_end(references: &[Option<usize>], start: usize) -> Option<usize> {
    let mut at = start;
    for _ in 0..references.len() {
        match references.get(at).copied().flatten() {
            Some(next) => at = next,
            None => return Some(at),
        }
    }
    None
}

/// The syntax the service documents for `Change$ChangeName`: the characters the
/// modelled `@pattern` admits with the modelled `@length(min: 1, max: 72)`
/// folded into it, which is how the service quotes it back to a caller.
///
/// The modelled `@pattern` (`^[a-zA-Z]$`) admits a SINGLE letter, which leaves
/// the modelled length's 72 nothing to describe, so the pattern on its own
/// cannot be what a caller is told to satisfy. The change-reference alternative
/// of [`ENTITY_IDENTIFIER_PATTERN`] states the same name as up to 72 letters,
/// and a reference can only name a change name a caller may send — so that is
/// the length a change name has here too.
pub const CHANGE_NAME_PATTERN: &str = "^[a-zA-Z]{1,72}$";

/// The `{1,72}` of [`CHANGE_NAME_PATTERN`].
const CHANGE_NAME_MAX: usize = 72;

/// Whether `value` is a `Change$ChangeName`: [`CHANGE_NAME_PATTERN`] spelled out
/// as a predicate — one to [`CHANGE_NAME_MAX`] ASCII letters.
pub fn is_change_name(value: &str) -> bool {
    (1..=CHANGE_NAME_MAX).contains(&value.chars().count())
        && value.chars().all(|c| c.is_ascii_alphabetic())
}

/// The modelled `ValidationException` payload for a `ChangeName` outside
/// [`CHANGE_NAME_PATTERN`].
pub fn invalid_change_name(name: &str) -> ValidationException {
    validation_message(quoting(
        "Requested change name '",
        name,
        &format!("' is invalid. It should match with {CHANGE_NAME_PATTERN}."),
    ))
}

/// The modelled `ValidationException` payload for a change set two of whose
/// changes carry the same `ChangeName`.
///
/// A change name is how one change of a change set names ANOTHER change
/// ([`change_reference_name`]), so a name two changes carry identifies neither:
/// like a repeated (entity, change type) pair, it is the request contradicting
/// itself, and the fault is built from the request — the name that was
/// repeated.
pub fn duplicate_change_name(change_name: &str) -> ValidationException {
    validation_message(format!(
        "Requested changes contain multiple changes with the same change name \
         '{change_name}'. Change names should be unique."
    ))
}

/// The modelled `ValidationException` payload for a change reference naming a
/// change the change set does not carry.
///
/// A reference is resolved against the changes of the SAME request, so a name
/// none of them carries names no entity — stated with the identifier the
/// request sent and the change name read out of it.
pub fn unknown_change_reference(identifier: &str, change_name: &str) -> ValidationException {
    validation_message(format!(
        "Requested entity identifier '{identifier}' names the change \
         '{change_name}', which no change of this request is named. A change \
         reference names a change of the same change set."
    ))
}

/// The modelled `ValidationException` payload for a change reference whose chain
/// of references leads back to a change it has already passed, so it names no
/// entity ([`reference_chain_end`]).
pub fn circular_change_reference(identifier: &str) -> ValidationException {
    validation_message(format!(
        "Requested entity identifier '{identifier}' names a change whose own \
         entity identifier leads back to it, so it names no entity."
    ))
}

/// The syntax the service documents for `ChangeSetName`: the modelled
/// `@pattern` with the modelled `@length(min: 1, max: 100)` folded into it,
/// which is how the service quotes it back to a caller.
pub const CHANGE_SET_NAME_PATTERN: &str = r"^[\w\s+=.:@-]{1,100}$";

/// The `{1,100}` of [`CHANGE_SET_NAME_PATTERN`].
const CHANGE_SET_NAME_MAX: usize = 100;

/// Whether `value` is a `ChangeSetName`: [`CHANGE_SET_NAME_PATTERN`] as a
/// predicate — one to [`CHANGE_SET_NAME_MAX`] characters of `\w` (an ASCII word
/// character), `\s` (whitespace) or `+=.:@-`.
///
/// `\w` is read as ASCII, not Unicode: the pattern is the one the service
/// publishes and its `\w` is the ASCII class, so a name of letters outside ASCII
/// is refused rather than quietly accepted against a wider class than the
/// pattern the caller was told to satisfy.
pub fn is_change_set_name(value: &str) -> bool {
    (1..=CHANGE_SET_NAME_MAX).contains(&value.chars().count())
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c.is_whitespace()
                || c == '_'
                || matches!(c, '+' | '=' | '.' | ':' | '@' | '-')
        })
}

/// The modelled `ValidationException` payload for a `ChangeSetName` outside
/// [`CHANGE_SET_NAME_PATTERN`].
pub fn invalid_change_set_name(name: &str) -> ValidationException {
    validation_message(quoting(
        "Requested change set name '",
        name,
        &format!("' is invalid. It should match with {CHANGE_SET_NAME_PATTERN}."),
    ))
}

/// The syntax the service documents for `ClientRequestToken`: the modelled
/// `@pattern` with the modelled `@length(min: 1, max: 64)` folded into it.
pub const CLIENT_REQUEST_TOKEN_PATTERN: &str = r"^[!-~]{1,64}$";

/// The `{1,64}` of [`CLIENT_REQUEST_TOKEN_PATTERN`].
const CLIENT_REQUEST_TOKEN_MAX: usize = 64;

/// Whether `value` is a `ClientRequestToken`: [`CLIENT_REQUEST_TOKEN_PATTERN`]
/// as a predicate — one to [`CLIENT_REQUEST_TOKEN_MAX`] printable ASCII
/// characters, space excluded (`!` to `~`).
pub fn is_client_request_token(value: &str) -> bool {
    (1..=CLIENT_REQUEST_TOKEN_MAX).contains(&value.chars().count())
        && value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
}

/// The modelled `ValidationException` payload for a `ClientRequestToken` outside
/// [`CLIENT_REQUEST_TOKEN_PATTERN`].
pub fn invalid_client_request_token(token: &str) -> ValidationException {
    validation_message(quoting(
        "Requested client request token '",
        token,
        &format!("' is invalid. It should match with {CLIENT_REQUEST_TOKEN_PATTERN}."),
    ))
}

/// The modelled `ValidationException` payload for a `NextToken` that names no
/// page of the list that was called.
///
/// A page token is OPAQUE: it is a value the service itself issued, so the fault
/// quotes neither the value nor a syntax — there is nothing a caller can correct
/// it to, only a page it has to ask for again. The same fault answers a token
/// outside the modelled `NextToken` syntax and one that is well formed but names
/// no page, because neither names a page.
pub fn invalid_page_token() -> ValidationException {
    validation_message("Requested page token is invalid.")
}

/// The form an entity id takes, as the model's own `EntityId` shape states it
/// (`@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")`).
///
/// The pattern states no length, and is reported verbatim in the refusal — the
/// form this service's own recorded refusals quote. The shape's
/// `@length(min: 1, max: 255)` ([`ENTITY_MEMBER_MAX`]) is a requirement all the
/// same, so [`is_entity_id`] enforces it beside this pattern rather than inside
/// it.
///
/// `EntityId` is the shape every member that NAMES an entity targets —
/// `EntityRequest$EntityId`, the `EntityDetails` / `Errors` map keys of
/// `BatchDescribeEntities`, `ValidationExceptionField$EntityId` — so it is this
/// service's statement of which strings are entity ids at all. A handful of
/// members, `DescribeEntityRequest$EntityId` among them, target the wider
/// `ResourceId` (`^[\w\-]+$`) instead, which admits strings no entity id can be
/// (an underscore, or a single character); the service enforces the `EntityId`
/// form on those members too, and the pattern is reported verbatim in the
/// refusal so a caller can see what was required.
pub const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// Whether `value` is an entity id: [`ENTITY_ID_PATTERN`], spelled out as a
/// predicate — an alphanumeric first and last character with one or more
/// alphanumeric, `.`, `/` or `-` characters between them (so the shortest entity
/// id is three characters long), the whole id within the modelled
/// [`ENTITY_MEMBER_MAX`].
pub fn is_entity_id(value: &str) -> bool {
    if value.chars().count() > ENTITY_MEMBER_MAX {
        return false;
    }
    let mut middle = value.chars();
    let (Some(first), Some(last)) = (middle.next(), middle.next_back()) else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && !middle.as_str().is_empty()
        && middle.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
}

/// The modelled `ValidationException` payload for an entity id outside
/// [`ENTITY_ID_PATTERN`] — what a read refuses BEFORE it looks the entity up, so
/// a malformed id is a 422 and only a well-formed one can reach the 404.
///
/// The message is built from the request: it names the rejected id and the
/// pattern the service requires, which is what lets a caller correct the call.
pub fn invalid_entity_id(entity_id: &str) -> ValidationException {
    validation_message(invalid_entity_id_message(entity_id))
}

/// The sentence that states `entity_id` is not an entity id, shared by the
/// top-level `ValidationException` a single entity read answers and the
/// `BatchDescribeErrorDetail$ErrorMessage` the batch read reports for the same
/// entry — so one read and a batch of them state a malformed id identically.
///
/// The id is quoted through [`quoting`], so however long a value was sent the
/// sentence stays inside the length of the member that carries it — the 2048 of
/// `ExceptionMessageContent` on one path and of
/// `BatchDescribeErrorMessageContent` on the other.
pub fn invalid_entity_id_message(entity_id: &str) -> String {
    quoting(
        "Requested entity id '",
        entity_id,
        &format!("' is invalid. It should match with {ENTITY_ID_PATTERN}."),
    )
}

/// The modelled `ValidationException` payload for a request value outside the
/// set this service publishes for the member that carried it.
///
/// The message is built from the request — the noun the value was asked for as,
/// the value sent, and the set it had to name — in the form this service states
/// such a refusal in (`[Requested sort by '…' is invalid. It should be one of
/// '…'.]`). Every closed-set refusal goes through here, so a caller reading any
/// of them reads the same sentence.
///
/// The value is quoted through [`quoting`], like every other sentence this
/// service builds around a submitted value: whatever was sent, the fault stays
/// the single line, inside the length, that the member carrying it declares.
pub fn invalid_choice(noun: &str, value: &str, supported: &[&str]) -> ValidationException {
    validation_message(quoting(
        &format!("Requested {noun} '"),
        value,
        &format!("' is invalid. It should be one of '{}'.", supported.join(", ")),
    ))
}

/// The modelled `ValidationException` payload for a `Sort$SortBy` outside the
/// attributes the list being called sorts on.
///
/// The sentence quotes the submitted attribute and then the set of attributes
/// the MODEL publishes for the member it is about: `Sort$SortBy` targets a plain
/// string with no enum constraint
/// (`smithy_query(query="shape", shape="Sort")`), so there is no published set
/// to name and the allowed-value slot renders the empty list — which is what the
/// service answers for either list. The attributes a request is JUDGED against
/// are the list's own ([`entity_sort_attributes`], [`CHANGE_SET_SORT_BY`]); they
/// are not published in the refusal, so a caller is never pointed at a set the
/// service does not declare.
///
/// The member is judged here rather than at the boundary (`skip_validation` on
/// `Sort`), so the attribute is quoted through [`quoting`]: however long a value
/// was sent, the fault stays inside the length its `Message` declares.
pub fn invalid_sort_by(sort_by: &str) -> ValidationException {
    validation_message(quoting(
        "Requested sort by '",
        sort_by,
        &format!("' is invalid. It should be one of '{UNPUBLISHED_VALUE_SET}'."),
    ))
}

/// How the allowed-value slot of a refusal renders a set the model does not
/// publish: the empty list.
const UNPUBLISHED_VALUE_SET: &str = "[]";

/// The modelled `ResourceNotFoundException` payload for a change set id that
/// names no change set.
///
/// Worded as this service words every "does not exist" refusal — naming the
/// resource the request asked for, which is the one rule a message built from the
/// request follows ("Requested change set '…' does not exist.", "Requested entity
/// '…' does not exist.", "Requested assessment '…' does not exist."). The error
/// code and the 404 are the contract; the prose follows that one form so a client
/// reading any of them reads the same sentence.
pub fn change_set_not_found(change_set_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Requested change set '{change_set_id}' does not exist."
        )),
    }
}

/// The modelled `ResourceNotFoundException` payload for an entity id that names
/// no entity.
///
/// The message is built from the request, naming the entity that was asked for,
/// in the form this service's recorded answers show it states a missing entity
/// in — the same sentence the `Errors` map of `BatchDescribeEntities` carries per
/// missing entity, so one entity read and a batch of them report a missing
/// entity identically.
pub fn entity_not_found(entity_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(entity_missing_message(entity_id)),
    }
}

/// The sentence that states `entity_id` names no entity, shared by the
/// `ResourceNotFoundException` a single entity read answers and the
/// `BatchDescribeErrorDetail$ErrorMessage` the batch read reports for the same
/// entity.
///
/// Both paths have judged the id an entity id before they look it up, so it is
/// inside that shape's `@length` and the sentence is inside its own without
/// cutting the quote.
pub fn entity_missing_message(entity_id: &str) -> String {
    format!("Requested entity '{entity_id}' does not exist.")
}

/// The `BatchDescribeErrorDetail$ErrorCode` for an entity the catalog does not
/// hold.
///
/// `BatchDescribeEntities` models no top-level error for a missing entity (its
/// modelled errors are AccessDenied / InternalService / Throttling / Validation
/// only) and answers one in the `Errors` map instead, keyed by the entity id,
/// carrying the name of the error a single read of that entity would have
/// raised.
pub const BATCH_ERROR_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `BatchDescribeErrorDetail$ErrorCode` for an entry whose `EntityId` is not
/// an entity id at all.
///
/// A batch read reports a malformed entry the same way it reports a missing one
/// — per entry, under the id that was asked for, carrying the name of the error a
/// single read of that entry would have raised — so one bad entry never costs the
/// caller the results for the good ones in the same batch.
pub const BATCH_ERROR_VALIDATION: &str = "ValidationException";

/// The modelled `ResourceNotFoundException` payload for a resource ARN that
/// names a resource of this catalog the store does not hold.
pub fn resource_not_found(resource: &CatalogResource) -> ResourceNotFoundException {
    match resource {
        CatalogResource::Entity { id, .. } => entity_not_found(id),
        CatalogResource::ChangeSet { id } => change_set_not_found(id),
        CatalogResource::Assessment { id } => assessment_not_found(id),
    }
}

/// The modelled `ResourceNotFoundException` payload a resource-policy operation
/// refuses a resource ARN with.
///
/// Each of those operations is documented for an ENTITY, so what such a call
/// asked for and did not find is an entity — whatever kind of resource the ARN
/// it was given names — and the refusal states that about the id the ARN named,
/// in the one form this service words a missing resource in
/// ([`entity_missing_message`]).
pub fn policy_resource_not_found(resource: &CatalogResource) -> ResourceNotFoundException {
    entity_not_found(resource.id())
}

/// The modelled `AccessDeniedException` payload for an action on a resource ARN
/// this catalog cannot authorize.
///
/// The message follows the authorization-failure form AWS states every such
/// refusal in — `User: <principal> is not authorized to perform: <action> on
/// resource: <resource>` — assembled from the authenticated principal
/// ([`caller_identity_arn`]), the IAM action (this service's `arnNamespace` is
/// its IAM service prefix, and `operation` the action name) and the resource the
/// request named ([`arn_resource_id`]). A client, a log scraper or an IAM
/// troubleshooter reads the denied principal out of that sentence.
pub fn access_denied(operation: &str, resource_arn: &str) -> AccessDeniedException {
    let resource = arn_resource_id(resource_arn);
    AccessDeniedException {
        message: Some(format!(
            "User: {} is not authorized to perform: {ARN_NAMESPACE}:{operation} on \
             resource: {resource} because no resource-based policy allows the \
             {ARN_NAMESPACE}:{operation} action",
            caller_identity_arn()
        )),
    }
}

/// The ARN of the identity every caller of this emulator authenticates as.
///
/// Every request is signed with the emulator's own credentials, so the caller is
/// the one IAM user this emulator publishes — the very principal its
/// `sts:GetCallerIdentity` answers, built the same way
/// (`::engine::emulator_arn("iam", "", "user/emulator")`), so the principal an
/// authorization refusal names is the principal the caller can look up.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// How this service states a member it refused for its SYNTAX: one bracketed
/// fault naming the member, the value it rejected and the syntax it required.
///
/// `member` is the input member's name as a violation path spells it
/// (lowerCamel); the label is that name as words, which is how the service's own
/// recorded refusals address a member — `catalog`, `entity type`, `change set
/// name`, `resource arn` — rather than by the wire path.
pub fn invalid_member_message(member: &str, value: &str, pattern: &str) -> String {
    quoting(
        &format!("[Requested {} '", member_label(member)),
        value,
        &format!("' is invalid. It should match with {pattern}.]"),
    )
}

/// An input member's name as the words a fault addresses it by: its camelCase
/// humps split apart and lowercased (`resourceArn` -> `resource arn`).
fn member_label(member: &str) -> String {
    let mut label = String::with_capacity(member.len() + 2);
    for (index, c) in member.char_indices() {
        if c.is_uppercase() && index != 0 {
            label.push(' ');
        }
        label.extend(c.to_lowercase());
    }
    label
}

/// The modelled `ResourceInUseException` payload for a resource another call
/// holds.
pub fn in_use(message: impl Into<String>) -> ResourceInUseException {
    ResourceInUseException {
        message: Some(message.into()),
    }
}

/// The modelled `ResourceInUseException` payload for an entity a still-running
/// change set holds, naming the entity the lock covers and the change set that
/// holds it.
///
/// `StartChangeSet` documents both the lock and this refusal: "when a ChangeSet
/// is running, all the entities targeted by the different changes are locked
/// until the change set has completed … If you try to start a change set
/// containing a change against an entity that is already locked, you will
/// receive a ResourceInUseException error." The holder comes from
/// [`ComAmazonawsMarketplacecatalogAwsmpseymourState::locking_change_set`], so
/// the id in the message is one the caller can read back with
/// `DescribeChangeSet`.
pub fn entity_locked(entity_id: &str, holder: &str) -> ResourceInUseException {
    in_use(format!(
        "entity {entity_id} is locked by the running change set {holder}"
    ))
}

/// The modelled `ResourceInUseException` payload for an entity an assessment is
/// still running over, naming the entity and the assessment that holds it.
///
/// An assessment is a run over ONE entity ("starts an assessment of a product
/// entity against a framework"), so while that run lasts the entity is in use:
/// a second start against the same entity is refused, stated the way the
/// sibling lock of this service is ([`entity_locked`]), with an assessment id
/// the caller can read back with `DescribeAssessment`.
pub fn assessment_in_progress(entity_id: &str, assessment_id: &str) -> ResourceInUseException {
    in_use(format!(
        "entity {entity_id} is being assessed by the running assessment {assessment_id}"
    ))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacecatalogAwsmpseymourState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacecatalogAwsmpseymourState>)
}
