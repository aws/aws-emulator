// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The event source types and their categories, as `DescribeEventCategories` reports
/// them.
///
/// The ORDER of this catalog is the order the report publishes, entry for entry and
/// category for category: neither the model nor the API reference states a sort rule for
/// either list, so the sequence is the front door's own published one and the handler emits
/// it unchanged — a client that indexes this report positionally, or renders it as the
/// ordered list it is, reads the source type the service names at that position. The
/// source-type sequence below, and the category sequence of the entries the front door's
/// own report is evidenced for (`db-snapshot`, `db-instance`, `db-cluster`, `db-proxy`), are
/// that published sequence; the remaining entries carry their categories in the order the
/// published event table lists them. It is NOT the order
/// `DescribeEventCategoriesMessage$SourceType`'s prose happens to name the types in, and
/// not an alphabetical sort of either list — no entry here is sorted, and an entry that
/// were would report a sequence the service never publishes.
///
/// The SETS are the ones published in "Amazon RDS event categories and event messages"
/// (Amazon RDS User Guide), one entry per source type this front door publishes a map for.
///
/// `db-instance` carries the two categories that table lists beyond the ones a DB
/// instance shares with the other source types — `backtrack` (an Aurora MySQL backtrack
/// event) and `security` — because a client builds a `CreateEventSubscription`
/// `EventCategories` argument out of this map and AWS accepts both for that source type;
/// `db-cluster-snapshot` carries `notification` beside `backup` for the same reason.
///
/// A source type the model's `SourceType` enum
/// (`smithy_query(query="shape",shape="SourceType")`) carries but this member's prose does
/// not list is published too, at the position the published report names it in, when this
/// emulator can cite the categories that source type publishes: `custom-engine-version` is
/// one — the published custom-engine-version event table carries a single `failure` event —
/// and a client that subscribes to a source type has to be able to read its categories from
/// this one map. The three remaining enum values — `blue-green-deployment`,
/// `db-shard-group` and `zero-etl` — have no published category table this emulator can
/// cite, so it publishes no map for them rather than an invented one, and a request naming
/// one is refused with the code a value this parameter does not accept gets, naming the
/// set it is not in.
pub const EVENT_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "db-snapshot",
        &["notification", "creation", "deletion", "restoration"],
    ),
    ("db-parameter-group", &["configuration change"]),
    (
        // The DB instance entry's own published sequence, which is NOT an alphabetical sort
        // of its categories: the report publishes each source type's categories in the order
        // that source type's own entry declares them, and this one declares them as the
        // published report names them.
        "db-instance",
        &[
            "backup",
            "deletion",
            "availability",
            "creation",
            "low storage",
            "restoration",
            "configuration change",
            "failover",
            "maintenance",
            "failure",
            "notification",
            "read replica",
            "recovery",
            "security",
            "backtrack",
            "security patching",
        ],
    ),
    ("db-security-group", &["configuration change", "failure"]),
    ("db-cluster-snapshot", &["backup", "notification"]),
    (
        "db-cluster",
        &[
            "failover",
            // The two categories only a CLUSTER publishes: it is migrated onto the
            // service (`migration`), and it runs serverless capacity (`serverless`).
            // Both are rows of the published DB cluster event table and both are
            // categories a cluster subscription may name.
            "migration",
            "failure",
            "notification",
            "serverless",
            "creation",
            "deletion",
            "maintenance",
            "configuration change",
            "global-failover",
        ],
    ),
    // The published custom-engine-version event table has one row, a failed CEV creation.
    ("custom-engine-version", &["failure"]),
    (
        "db-proxy",
        &["creation", "configuration change", "deletion", "failure"],
    ),
];

/// The event categories of `source_type`, or `None` when it names no published source
/// type. Matching is exact: the documented values are all lowercase.
pub fn event_categories_of(source_type: &str) -> Option<&'static [&'static str]> {
    EVENT_CATEGORIES
        .iter()
        .find(|(published, _)| *published == source_type)
        .map(|(_, categories)| *categories)
}

/// The message the refusal of a `SourceType` that names no published event source type
/// carries.
///
/// The set of source types the front door accepts is wider than the ones this report
/// publishes a category map for, and the refusal names the whole of it, so it is the shared
/// `service_common::rds_event_source_types` list — one set for every service view this one
/// front door serves — that this sentence comes from.
pub fn invalid_event_source_type_message() -> String {
    ::service_common::rds_event_source_types::invalid_event_source_type_message()
}

/// One source type's categories as the modelled `EventCategoriesMap`.
pub fn event_categories_to_wire(source_type: &str, categories: &[&str]) -> EventCategoriesMap {
    EventCategoriesMap {
        source_type: Some(source_type.to_string()),
        event_categories: Some(
            categories
                .iter()
                .map(|category| (*category).to_string())
                .collect(),
        ),
    }
}

/// The source types an event notification SUBSCRIPTION may name.
///
/// The set is the one the model's `SourceType` enum carries
/// (`crate::ops::types::SOURCE_TYPE_VALUES`), which is `db-shard-group` wider than the
/// list `CreateEventSubscriptionMessage$SourceType`'s prose spells out; the ORDER is the
/// one Amazon RDS lists the valid types in when it refuses a value outside the set
/// ([`invalid_event_subscription_source_type_message`]), so the refusal names them exactly
/// as the service does.
///
/// Wider than the source types [`EVENT_CATEGORIES`] carries a category map for, which is
/// the narrower list `DescribeEventCategories`'s own member documents: a subscription may
/// be created for a source type whose published categories this emulator does not have,
/// and [`check_event_categories`] says what that means for the categories such a
/// subscription may name.
pub const EVENT_SUBSCRIPTION_SOURCE_TYPES: [&str; 11] = [
    "db-instance",
    "db-security-group",
    "db-parameter-group",
    "db-snapshot",
    "db-cluster",
    "db-cluster-snapshot",
    "custom-engine-version",
    "db-proxy",
    "blue-green-deployment",
    "db-shard-group",
    "zero-etl",
];

/// The message Amazon RDS refuses a `SourceType` no subscription can be created for with.
///
/// A statement about the PARAMETER and not about the value: the fixed sentence says the
/// source type is not valid and then names the types that ARE valid — it echoes nothing
/// from the request, so neither does this. Each type is named in single quotes, the way the
/// service writes a list of literal values inside a sentence, in the order
/// [`EVENT_SUBSCRIPTION_SOURCE_TYPES`] carries them, and the sentence ends with that list
/// (no trailing period). The sibling refusal a `DescribeEvents` source type gets is worded
/// with one more clause ([`invalid_event_source_type_message`]) and over the narrower set
/// that report covers.
pub fn invalid_event_subscription_source_type_message() -> String {
    let quoted: Vec<String> = EVENT_SUBSCRIPTION_SOURCE_TYPES
        .iter()
        .map(|source_type| format!("'{source_type}'"))
        .collect();
    format!("Invalid event source type. Valid types are {}", quoted.join(", "))
}

/// How Amazon RDS refuses a request that names event categories without the source type
/// they belong to: ONE fixed sentence, which is why it is a constant — it echoes neither
/// the categories nor the subscription, so the same text answers every such request.
pub const CATEGORIES_NEED_A_SOURCE_TYPE: &str =
    "If SourceType is null, Categories must also be null.";

/// The constraint `CreateEventSubscriptionMessage$SourceIds` states in those words: "If
/// `SourceIds` are supplied, `SourceType` must also be provided." The ids name resources of
/// one source type, so there is nothing to resolve them against without it.
pub const SOURCE_IDS_NEED_A_SOURCE_TYPE: &str =
    "If SourceIds are supplied, SourceType must also be provided.";

/// Refuse a `SourceType` a subscription cannot be created for; the caller answers it with
/// the Query API's `InvalidParameterValue`.
pub fn check_event_subscription_source_type(source_type: &str) -> Result<(), String> {
    if EVENT_SUBSCRIPTION_SOURCE_TYPES.contains(&source_type) {
        return Ok(());
    }
    Err(invalid_event_subscription_source_type_message())
}

/// The `SourceNotFoundFault` this service answers a request that names an event source
/// which does not exist, built HERE so every handler that resolves a source identifier
/// words the one fault the same way.
///
/// Its text is fixed — `Could not find source :` — and its only variable part is the
/// identifier the request supplied, carried verbatim: RDS interpolates neither the source
/// type the identifier was resolved against nor the region the call was signed for, and
/// appends no trailing period.
pub fn source_not_found(source_identifier: &str) -> SourceNotFoundFault {
    SourceNotFoundFault {
        message: Some(format!("Could not find source :{source_identifier}")),
    }
}

/// The categories of `source_type` a subscription may subscribe to, with the one that
/// is not among them, if any: `EventCategories` is "a list of event categories for a
/// particular source type (SourceType) that you want to subscribe to", so a category
/// that source type does not publish names no events at all.
///
/// A source type [`EVENT_CATEGORIES`] carries no published map for is not checked —
/// there is nothing to check it against, and refusing every category for it would refuse
/// the ones AWS accepts. Matching is exact: the published categories are all lowercase.
pub fn check_event_categories(source_type: &str, categories: &[String]) -> Result<(), String> {
    let Some(published) = event_categories_of(source_type) else {
        return Ok(());
    };
    for category in categories {
        if !published.contains(&category.as_str()) {
            return Err(format!(
                "{category} is not a published event category of {source_type} \
                 (its categories are {})",
                published.join(", ")
            ));
        }
    }
    Ok(())
}

