// SPDX-License-Identifier: CC0-1.0

//! The product resource: the record the product handlers share, the forms of the
//! identifiers the service mints for it, the rules its `apiSelectionConfiguration`
//! must satisfy, and the shapes a response reports it in.
//!
//! A product is the primary resource of API Portal: it groups Managed APIs with their
//! API pages and product pages so a portal can publish them together. `CreateProduct`
//! writes one record here, `GetProduct`/`ListProducts` read it, `UpdateProduct` merges
//! into it and `DeleteProduct` removes it, so every one of those answers is computed
//! from this one record.

use std::collections::{BTreeMap, BTreeSet};
use std::collections::HashMap;

use crate::ops::types::{
    ApiSelectionConfiguration, DisplayOrder, ProductSummary, RuleFilter,
    API_SELECTION_MODE_VALUES,
};

/// `ACTIVE` — the product's member APIs match its `apiSelectionConfiguration`.
///
/// From `smithy_query(query="shape", shape="ProductStatus")`:
/// `ACTIVE | IN_PROGRESS | FAILED`, documented as "ACTIVE - membership matches. The
/// only status a CUSTOM product reports."
pub const PRODUCT_STATUS_ACTIVE: &str = "ACTIVE";

/// `IN_PROGRESS` — "ruleFilters changed and membership is still being evaluated",
/// per the same `ProductStatus` documentation.
pub const PRODUCT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `CUSTOM` — "you associate APIs yourself", from
/// `smithy_query(query="shape", shape="ApiSelectionConfiguration")`. The mode a
/// product takes when a create supplies no `apiSelectionConfiguration`: it is the only
/// mode that needs no `ruleFilters`, and every product has a mode because the
/// documentation says the mode "can't be changed after the product is created".
pub const API_SELECTION_MODE_CUSTOM: &str = "CUSTOM";

/// `RULE_BASED` — "the APIs that match `ruleFilters` are selected", from the same
/// `ApiSelectionConfiguration` documentation.
pub const API_SELECTION_MODE_RULE_BASED: &str = "RULE_BASED";

/// How long after a create or update that sets `ruleFilters` the rule evaluation the
/// clock owes falls due, i.e. how long a `RULE_BASED` product stays `IN_PROGRESS`.
///
/// `ProductStatus` gives a product the non-terminal `IN_PROGRESS` status for exactly
/// this window ("ruleFilters changed and membership is still being evaluated"), so a
/// caller polling straight after the call must be able to observe it. No AWS document
/// fixes the span, so this is a CHOSEN value; it is never zero.
pub const RULE_EVALUATION_MILLIS: i64 = 3_000;

/// The page size `ListProducts` uses when the request supplies no `maxResults`, and
/// the ceiling it clamps a supplied one to.
///
/// A listing is always bounded: the model bounds `maxResults` to 1..100
/// (`smithy_query(query="operation", operation="ListProducts")`), so an absent one
/// pages at that same maximum and hands back a `nextToken` rather than collecting the
/// whole store into one response.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The most products this emulator's store will hold.
///
/// A bound on the store rather than an AWS-published quota: nothing else limits how
/// many products a caller can create, and an in-memory store on a developer's machine
/// must not grow without one. Beyond this `CreateProduct` answers the
/// `LimitExceededException` it models.
pub const MAX_PRODUCTS: usize = 200;

/// `SELF` — "your own" products, from
/// `smithy_query(query="doc", shape="ListProductsRequest", member="resourceOwner")`.
pub const RESOURCE_OWNER_SELF: &str = "SELF";

/// `OTHER_ACCOUNTS` — "products shared with you", from the same documentation.
pub const RESOURCE_OWNER_OTHER_ACCOUNTS: &str = "OTHER_ACCOUNTS";

/// `ALL` — both, and the default "or `SELF` when `managedApiId` is supplied".
pub const RESOURCE_OWNER_ALL: &str = "ALL";

/// Whether `account_id` is the one account whose resources this emulator holds.
///
/// The emulator presents exactly one account — `engine::emulator_account_id()` — so a
/// request naming any other account names an account it holds no products for.
pub fn is_emulator_account(account_id: &str) -> bool {
    account_id == ::engine::emulator_account_id()
}

/// The `ResourceNotFoundException` message for a product the store does not hold.
///
/// `ResourceNotFoundException` is documented as "The requested resource does not exist,
/// or you do not have access to it."
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`); the product the
/// request named is what makes it actionable.
pub fn product_not_found_message(product_id: &str) -> String {
    format!("Product {product_id} does not exist, or you do not have access to it.")
}

/// The `productId` for the `sequence`-th product this store minted.
///
/// `ProductId` is `@length(min: 10, max: 30)` with pattern `^[a-z0-9]+$`
/// (`smithy_query(query="shape", shape="ProductId")`), which this form satisfies: one
/// lowercase letter and nine lowercase hex digits, ten characters in all. It is drawn
/// from a monotonic counter, so an id is unique for the life of the store and no
/// later create reuses a deleted product's id.
pub fn product_id(sequence: u64) -> String {
    format!("p{sequence:09x}")
}

/// The `productArn` of `product_id` in `region`.
///
/// `ProductArn`'s pattern is
/// `^arn:aws[a-z\-]*:apiportal:[a-z0-9\-]+:[0-9]{12}:products/[a-z0-9]{10,30}$`
/// (`smithy_query(query="shape", shape="ProductArn")`), so the resource portion is
/// `products/<productId>`; the partition and account segments come from the emulator's
/// own ARN builder.
pub fn product_arn(region: &str, product_id: &str) -> String {
    ::engine::emulator_arn("apiportal", region, &format!("products/{product_id}"))
}

/// Whether two `ruleFilters` lists select the same Managed APIs.
///
/// `RuleFilter` carries no equality impl, and a list is "replaced as a whole on update,
/// not merged", so an update is a CHANGE only when the replacement differs — which is
/// what decides whether the product owes another evaluation.
fn rule_filters_match(left: Option<&Vec<RuleFilter>>, right: Option<&Vec<RuleFilter>>) -> bool {
    let key = |filters: Option<&Vec<RuleFilter>>| -> Vec<(String, Vec<String>)> {
        filters
            .map(|filters| {
                filters
                    .iter()
                    .map(|filter| (filter.key.clone(), filter.values.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    key(left) == key(right)
}

/// Refuse an `apiSelectionConfiguration` the service's own rules reject, naming the
/// member at fault; `Ok(())` when it is one the service accepts.
///
/// The rule is the model's own, from
/// `smithy_query(query="doc", shape="ApiSelectionConfiguration", member="ruleFilters")`:
/// "Required for RULE_BASED, rejected for CUSTOM."
pub fn validate_api_selection_configuration(
    configuration: &ApiSelectionConfiguration,
) -> Result<(), String> {
    let mode = configuration.api_selection_mode.as_str();
    if !API_SELECTION_MODE_VALUES.contains(&mode) {
        return Err(format!(
            "The value of apiSelectionConfiguration.apiSelectionMode is not valid: {mode}. \
             Valid values are {}.",
            API_SELECTION_MODE_VALUES.join(", ")
        ));
    }
    let has_rule_filters = configuration
        .rule_filters
        .as_ref()
        .is_some_and(|filters| !filters.is_empty());
    if mode == API_SELECTION_MODE_RULE_BASED && !has_rule_filters {
        return Err(
            "The value of apiSelectionConfiguration.ruleFilters is not valid: ruleFilters is \
             required when apiSelectionMode is RULE_BASED."
                .to_string(),
        );
    }
    if mode == API_SELECTION_MODE_CUSTOM && has_rule_filters {
        return Err(
            "The value of apiSelectionConfiguration.ruleFilters is not valid: ruleFilters is \
             rejected when apiSelectionMode is CUSTOM."
                .to_string(),
        );
    }
    Ok(())
}

/// One product, as the store holds it.
#[derive(Debug, Clone)]
pub struct Product {
    pub product_id: String,
    pub product_arn: String,
    pub display_name: String,
    /// `ProductDescription`. Absent until a create or update supplies one; the
    /// `GetProduct` and `ProductSummary` shapes make `description` required, so a read
    /// reports the empty string for a product that has none.
    pub description: Option<String>,
    /// `DisplayOrder`. Absent until an update supplies one — `CreateProduct` models no
    /// `displayOrder` input, so a new product has no page order yet.
    pub display_order: Option<DisplayOrder>,
    /// How this product's Managed APIs are selected. Always present: the mode "can't be
    /// changed after the product is created", so every product has one from the create
    /// onward — [`API_SELECTION_MODE_CUSTOM`] when the create supplied none.
    pub api_selection_configuration: ApiSelectionConfiguration,
    /// A `ProductStatus`; see [`PRODUCT_STATUS_ACTIVE`] / [`PRODUCT_STATUS_IN_PROGRESS`].
    pub status: String,
    /// A `ProductFailureReason`, "present only when `status` is `FAILED`". No path in
    /// this slice fails an evaluation, so it stays `None`.
    pub failure_reason: Option<String>,
    pub updated_at_millis: i64,
    /// When the `IN_PROGRESS` -> `ACTIVE` transition falls due. Stored, never slept on:
    /// [`Product::settle`] materialises it on the next call at or after this instant.
    pub evaluation_due_millis: i64,
    pub tags: BTreeMap<String, String>,
    /// The Managed APIs that are members of this product, as the store holds them.
    ///
    /// Membership is what `ListProducts`' `managedApiId` filter selects on. The
    /// emulator holds no Managed API records yet — every operation that would create or
    /// associate one is still scaffolded — so an evaluation over "the account's Managed
    /// APIs" selects none and this set is empty.
    pub member_managed_api_ids: BTreeSet<String>,
}

impl Product {
    /// A product as `CreateProduct` mints it: `ACTIVE` when its APIs are selected by
    /// hand, and `IN_PROGRESS` until the rule evaluation its `ruleFilters` owe falls
    /// due.
    pub fn new(
        product_id: String,
        product_arn: String,
        display_name: String,
        description: Option<String>,
        api_selection_configuration: ApiSelectionConfiguration,
        tags: BTreeMap<String, String>,
        now_millis: i64,
    ) -> Self {
        let mut product = Self {
            product_id,
            product_arn,
            display_name,
            description,
            display_order: None,
            api_selection_configuration,
            status: PRODUCT_STATUS_ACTIVE.to_string(),
            failure_reason: None,
            updated_at_millis: now_millis,
            evaluation_due_millis: now_millis,
            tags,
            member_managed_api_ids: BTreeSet::new(),
        };
        if product.is_rule_based() {
            product.start_rule_evaluation(now_millis);
        }
        product
    }

    /// Whether this product's membership comes from `ruleFilters`.
    pub fn is_rule_based(&self) -> bool {
        self.api_selection_configuration.api_selection_mode == API_SELECTION_MODE_RULE_BASED
    }

    /// Owe another membership evaluation: the product reports `IN_PROGRESS` until it
    /// falls due, which is when its members are re-selected.
    fn start_rule_evaluation(&mut self, now_millis: i64) {
        self.status = PRODUCT_STATUS_IN_PROGRESS.to_string();
        self.failure_reason = None;
        self.evaluation_due_millis = now_millis + RULE_EVALUATION_MILLIS;
        // Membership is selected afresh from the account's Managed APIs, of which the
        // store holds none, so the evaluation selects nothing.
        self.member_managed_api_ids.clear();
    }

    /// Materialise the transition the clock has already made due at `now_millis`.
    ///
    /// A product whose evaluation came due is `ACTIVE` from that instant onward whether
    /// or not anybody called, so its `updatedAt` is the instant the transition fell due.
    pub fn settle(&mut self, now_millis: i64) {
        if self.status == PRODUCT_STATUS_IN_PROGRESS && self.evaluation_due_millis <= now_millis {
            self.status = PRODUCT_STATUS_ACTIVE.to_string();
            self.updated_at_millis = self.evaluation_due_millis;
        }
    }

    /// Apply the members an `UpdateProduct` request supplied, leaving every absent one
    /// at its stored value, and bump `updatedAt`.
    ///
    /// A replacement `apiSelectionConfiguration` whose `ruleFilters` differ owes another
    /// evaluation, which is what puts the product back to `IN_PROGRESS`.
    pub fn apply_update(
        &mut self,
        display_name: Option<String>,
        description: Option<String>,
        display_order: Option<DisplayOrder>,
        api_selection_configuration: Option<ApiSelectionConfiguration>,
        now_millis: i64,
    ) {
        if let Some(display_name) = display_name {
            self.display_name = display_name;
        }
        if let Some(description) = description {
            self.description = Some(description);
        }
        if let Some(display_order) = display_order {
            self.display_order = Some(display_order);
        }
        self.updated_at_millis = now_millis;
        let Some(replacement) = api_selection_configuration else {
            return;
        };
        let changed = !rule_filters_match(
            replacement.rule_filters.as_ref(),
            self.api_selection_configuration.rule_filters.as_ref(),
        );
        self.api_selection_configuration = replacement;
        if changed && self.is_rule_based() {
            self.start_rule_evaluation(now_millis);
        }
    }

    /// Whether `managed_api_id` is one of this product's member Managed APIs — what
    /// `ListProducts`' `managedApiId` filter selects on.
    pub fn has_member_managed_api(&self, managed_api_id: &str) -> bool {
        self.member_managed_api_ids.contains(managed_api_id)
    }

    /// `updatedAt`, in the modelled timestamp shape.
    pub fn updated_at(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_millis(self.updated_at_millis)
    }

    /// `tags`, or `None` when the product carries none: `Tags` is
    /// `@length(min: 1, max: 50)`, so an empty map is not a value the shape admits.
    pub fn tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }

    /// `description` as a shape that requires it reports it: the empty string for a
    /// product that has none, which `ProductDescription`'s `@length(min: 0)` admits.
    pub fn description_or_empty(&self) -> String {
        self.description.clone().unwrap_or_default()
    }

    /// `displayOrder` as a shape that requires it reports it: a `DisplayOrder` with no
    /// pages ordered yet, for a product that has none.
    pub fn display_order_or_empty(&self) -> DisplayOrder {
        self.display_order.clone().unwrap_or_default()
    }

    /// `apiSelectionConfiguration`, as a response reports it.
    pub fn api_selection_configuration(&self) -> Option<ApiSelectionConfiguration> {
        Some(self.api_selection_configuration.clone())
    }

    /// This product as one `ListProducts` item.
    pub fn summary(&self) -> ProductSummary {
        ProductSummary {
            product_id: self.product_id.clone(),
            product_arn: self.product_arn.clone(),
            display_name: self.display_name.clone(),
            description: self.description_or_empty(),
            updated_at: self.updated_at(),
            api_selection_configuration: self.api_selection_configuration(),
            status: Some(self.status.clone()),
            failure_reason: self.failure_reason.clone(),
        }
    }
}
