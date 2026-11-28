// SPDX-License-Identifier: CC0-1.0

//! The stored fields a customer-account search reads: what each `FilterableField`,
//! `SortableField` and `AggregatableField` holds for one account, and how an account's
//! order is decided.
//!
//! `SearchCustomerAccounts` and `GetCustomerAccountStatistics` read the same fields, so
//! the table lives here rather than in either operation's file. Everything a REQUEST
//! carries — the `Filter` tree's bounds and evaluation, and the opaque page token —
//! lives with the search operation that accepts it
//! ([`crate::ops::search_customer_accounts`]); nothing here reads a caller's string: it
//! matches an enum value the boundary already validated and reads stored values back.

use super::StoredCustomerAccount;
use crate::ops::types::{AwsEnrichmentCustomer, AwsEnrichmentInsights, CustomerAccountSort};

/// `SortableField` the emulator orders by when a request carries no `Sort`. The model
/// documents no default, so this is the emulator's choice: the most recently modified
/// account first, as the service's other paginated read documents.
const DEFAULT_SORT_BY: &str = "LAST_MODIFIED_DATE";

/// What one field holds for one account. The kind decides which comparisons the field
/// accepts: a date field takes the range conditions and a `DateValue`, and a field that
/// holds more than one value is the only one `ContainsAll` applies to.
pub enum FieldValue<'a> {
    Text(Option<&'a str>),
    MultiText(Vec<&'a str>),
    /// An instant, as epoch milliseconds.
    Date(i64),
}

/// What `key` — a `FilterableField` or an `AggregatableField` — holds for `account`,
/// or `None` when it names no field of either enum.
///
/// The fields AWS classifies (industry, segment, region, geo, country, the scores, the
/// programs, the account-planning qualification) come from the account's
/// `AwsEnrichment`, present only when matching enriched it; `IMPORT_ID` names the bulk
/// import that created the account, and one created through `CreateCustomerAccount` was
/// created by no import. Those read as absent, which is what makes a condition on them
/// match nothing and a group over them empty.
pub fn field_value<'a>(account: &'a StoredCustomerAccount, key: &str) -> Option<FieldValue<'a>> {
    let customer = account
        .aws_enrichment
        .as_ref()
        .and_then(|enrichment| enrichment.customer.as_ref());
    let insights = account
        .aws_enrichment
        .as_ref()
        .and_then(|enrichment| enrichment.insights.as_ref());
    let customer_text = |read: fn(&AwsEnrichmentCustomer) -> Option<&String>| {
        FieldValue::Text(customer.and_then(read).map(String::as_str))
    };
    let insights_text = |read: fn(&AwsEnrichmentInsights) -> Option<&String>| {
        FieldValue::Text(insights.and_then(read).map(String::as_str))
    };

    Some(match key {
        "ENRICHMENT_STATUS" => FieldValue::Text(Some(account.enrichment_status.as_str())),
        "COMPANY_NAME" => FieldValue::Text(Some(account.account.company_name.as_str())),
        "IMPORT_ID" => FieldValue::Text(None),
        "CREATED_DATE" => FieldValue::Date(account.created_date_ms),
        "LAST_MODIFIED_DATE" => FieldValue::Date(account.last_modified_date_ms),
        "AWS_INDUSTRY" => customer_text(|customer| customer.industry.as_ref()),
        "SEGMENT" => customer_text(|customer| customer.segment.as_ref()),
        "REGION" => customer_text(|customer| customer.region.as_ref()),
        "GEO" => customer_text(|customer| customer.geo.as_ref()),
        "COUNTRY" => customer_text(|customer| customer.country.as_ref()),
        "ACCOUNT_PLANNING_QUALIFICATION" => {
            customer_text(|customer| customer.customer_account_planning_qualification.as_ref())
        }
        "SOLUTION_CATEGORY" => insights_text(|insights| insights.solution_category.as_ref()),
        "MARKETPLACE_ENGAGEMENT_SCORE" => {
            insights_text(|insights| insights.marketplace_engagement_score.as_ref())
        }
        "SOLUTION_SCORE" => insights_text(|insights| insights.solution_score.as_ref()),
        "PROGRAM_ELIGIBILITY" => FieldValue::MultiText(
            customer
                .and_then(|customer| customer.eligible_programs.as_ref())
                .map(|programs| programs.iter().map(String::as_str).collect())
                .unwrap_or_default(),
        ),
        _ => return None,
    })
}

/// Order `accounts` as `Sort` asks.
///
/// Every comparison breaks ties on the id, so the order is total: paging through it can
/// neither repeat nor skip an account, and a descending order is the exact reverse of
/// the ascending one. A score field orders "High, Medium, Low, with accounts that have
/// no score last", so a missing score stays last whichever direction the scores
/// themselves take.
pub fn sort_accounts(
    accounts: &mut [&StoredCustomerAccount],
    sort: Option<&CustomerAccountSort>,
) {
    let sort_by = sort.map_or(DEFAULT_SORT_BY, |sort| sort.sort_by.as_str());
    let descending = sort.is_none_or(|sort| sort.sort_order != "ASCENDING");
    let direct = |ordering: ::std::cmp::Ordering| {
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    };
    accounts.sort_by(|left, right| match sort_by {
        "MARKETPLACE_ENGAGEMENT_SCORE" | "SOLUTION_SCORE" => {
            let left_rank = score_rank(left, sort_by);
            let right_rank = score_rank(right, sort_by);
            left_rank
                .is_none()
                .cmp(&right_rank.is_none())
                .then_with(|| {
                    direct(
                        left_rank
                            .cmp(&right_rank)
                            .then_with(|| left.id.cmp(&right.id)),
                    )
                })
        }
        _ => {
            let ascending = match sort_by {
                "COMPANY_NAME" => left.account.company_name.cmp(&right.account.company_name),
                "CREATED_DATE" => left.created_date_ms.cmp(&right.created_date_ms),
                _ => left.last_modified_date_ms.cmp(&right.last_modified_date_ms),
            };
            direct(ascending.then_with(|| left.id.cmp(&right.id)))
        }
    });
}

/// Where a score field's value ranks: "Scores sort in the order `High`, `Medium`,
/// `Low`". `None` is an account with no score.
fn score_rank(account: &StoredCustomerAccount, field: &str) -> Option<u8> {
    let value = match field_value(account, field) {
        Some(FieldValue::Text(value)) => value,
        _ => None,
    };
    match value? {
        "High" => Some(0),
        "Medium" => Some(1),
        "Low" => Some(2),
        _ => None,
    }
}

/// The values of `group_by` for one account: one value for a single-valued field, every
/// value for a multi-valued one, and an empty list when the account has no value for it.
///
/// `None` when `group_by` names no field customer accounts can be grouped by — a date
/// field included, since `AggregatableField` declares none.
pub fn group_values(account: &StoredCustomerAccount, group_by: &str) -> Option<Vec<String>> {
    match field_value(account, group_by)? {
        FieldValue::Text(value) => Some(value.map(ToString::to_string).into_iter().collect()),
        FieldValue::MultiText(values) => {
            Some(values.into_iter().map(ToString::to_string).collect())
        }
        FieldValue::Date(_) => None,
    }
}
