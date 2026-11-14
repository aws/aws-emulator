// SPDX-License-Identifier: CC0-1.0

//! The records the Amazon Personalize Events data plane ingested, read as the ROWS of a
//! dataset.
//!
//! Amazon Personalize holds one dataset whichever way its records arrived: "you can
//! choose to export only BULK data that you imported using a dataset import job, only
//! PUT data that you imported incrementally (using the console, PutEvents, PutUsers and
//! PutItems operations), or ALL for both types"
//! (`smithy_query(query="doc", shape="DatasetExportJob", member="ingestionMode")`). The
//! BULK half of a dataset is already rows ([`crate::state::DatasetRecord::imported`]);
//! this module is the PUT half, turned into the same [`Row`] shape so everything that
//! reads a dataset's records — training, a dataset export job — reads one representation
//! of them instead of each walking the ingestion store its own way.
//!
//! Nothing is invented here: every value comes from a record the ingestion data plane
//! actually stored.

use std::collections::BTreeMap;

use ::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state as ingestion;

use crate::state::import::{Row, ACTION_ID_COLUMN, ITEM_ID_COLUMN, USER_ID_COLUMN};
use crate::state::{is_dataset_type, DatasetRecord};

/// The column of an interaction row carrying the instant of the interaction, in the
/// Unix time the records themselves are kept in.
///
/// Source: the fields Amazon Personalize requires of an Interactions and an Action
/// interactions schema, which this crate states once in
/// [`crate::state::REQUIRED_SCHEMA_FIELDS`] — `timestamp` and `event_type` beside the
/// user, item and action ids the other three constants name.
pub const TIMESTAMP_COLUMN: &str = "timestamp";
/// The column naming the KIND of interaction (same source as [`TIMESTAMP_COLUMN`]).
pub const EVENT_TYPE_COLUMN: &str = "event_type";
/// The column carrying `Event$eventValue`, "the value of the event" an interaction
/// reports when the caller sent one.
pub const EVENT_VALUE_COLUMN: &str = "event_value";

/// The string-valued members of a streamed metadata record's `properties` object.
///
/// A property whose value is not a string is rendered as the JSON it was ingested as, so
/// a reader sees the value that was actually put rather than nothing at all.
pub fn properties_of(record: &ingestion::Record) -> Vec<(String, String)> {
    columns_of(record.properties.as_ref())
}

/// The same for the `properties` of an ingested EVENT, which is the same JSON object
/// member on a different record.
fn columns_of(properties: Option<&::serde_json::Value>) -> Vec<(String, String)> {
    let Some(::serde_json::Value::Object(properties)) = properties else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(name, value)| {
            let rendered = match value {
                ::serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (name.clone(), rendered)
        })
        .collect()
}

/// The rows the ingestion data plane holds for `dataset`: the records put into it
/// (`PutItems` / `PutUsers` / `PutActions`) for a dataset holding entities, and the
/// events its dataset group's own trackers ingested (`PutEvents` /
/// `PutActionInteractions`) for a dataset holding a stream.
///
/// `ingested` is the Amazon Personalize Events store when this engine carries it; an
/// engine without it holds no incrementally ingested records at all, so the answer is
/// empty rather than invented.
///
/// A pure read of that store, in its own stable key order, so two readers agree.
///
/// At most `limit` rows are built, so the caller's own ceiling bounds the work and the
/// allocation rather than the store's retention (which bounds one dataset and one
/// tracker, while a dataset group holds many trackers). A caller that has to know
/// whether MORE rows exist passes one more than its ceiling and reads the length.
pub fn put_rows(
    ingested: Option<&ingestion::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState>,
    dataset: &DatasetRecord,
    limit: usize,
) -> Vec<Row> {
    let Some(ingested) = ingested else {
        return Vec::new();
    };

    // An Items, Users or Actions dataset holds one record per entity, which the metadata
    // puts address by the dataset's own ARN.
    if let Some((kind, id_column)) = entity_collection(dataset) {
        let Some(streamed) = ingested.datasets.get(&dataset.dataset_arn) else {
            return Vec::new();
        };
        return entity_rows(streamed.records(kind), id_column, limit);
    }

    // An interactions dataset holds the STREAM its dataset group's trackers ingested: a
    // tracking id names a tracker, never a dataset, so the group is what ties the two
    // together (the same join `crate::state::training` reads).
    let trackers = ingested.event_trackers.values().filter(|tracker| {
        tracker.dataset_group_arn.as_deref() == Some(dataset.dataset_group_arn.as_str())
    });
    let mut rows = Vec::new();
    if is_dataset_type(dataset, "Interactions") {
        for tracker in trackers {
            if rows.len() >= limit {
                break;
            }
            rows.extend(
                tracker
                    .events
                    .iter()
                    .take(limit - rows.len())
                    .map(event_row),
            );
        }
    } else if is_dataset_type(dataset, "Action_Interactions") {
        for tracker in trackers {
            if rows.len() >= limit {
                break;
            }
            rows.extend(
                tracker
                    .action_interactions
                    .iter()
                    .take(limit - rows.len())
                    .map(action_interaction_row),
            );
        }
    }
    rows
}

/// Which collection of the ingestion store holds `dataset`'s records, and the column its
/// records are keyed by — or `None` for a dataset holding a stream rather than entities.
fn entity_collection(dataset: &DatasetRecord) -> Option<(ingestion::MetadataKind, &'static str)> {
    if is_dataset_type(dataset, "Items") {
        Some((ingestion::MetadataKind::Items, ITEM_ID_COLUMN))
    } else if is_dataset_type(dataset, "Users") {
        Some((ingestion::MetadataKind::Users, USER_ID_COLUMN))
    } else if is_dataset_type(dataset, "Actions") {
        Some((ingestion::MetadataKind::Actions, ACTION_ID_COLUMN))
    } else {
        None
    }
}

/// Merge the `properties` a record carries into `row`, DROPPING any whose name is one of
/// the `derived` columns the row takes from the stored record itself.
///
/// What identifies a record, and when its interaction happened, come from the record — the
/// fields a schema of its dataset type requires (`crate::state::REQUIRED_SCHEMA_FIELDS`)
/// plus the event's own value — so a property a caller happens to name the same as one of
/// them cannot reshape the row's identity or its instant. Names are compared the way every
/// field name in this crate is: case-insensitively.
fn merge_properties(row: &mut Row, derived: &[&str], properties: Option<&::serde_json::Value>) {
    for (name, value) in columns_of(properties) {
        if derived.iter().any(|column| column.eq_ignore_ascii_case(&name)) {
            continue;
        }
        row.insert(name, value);
    }
}

/// One row per stored entity: the id it is keyed by, in the column that names it, and
/// one column per property that was put with it. At most `limit` of them.
fn entity_rows(
    records: &BTreeMap<String, ingestion::Record>,
    id_column: &str,
    limit: usize,
) -> Vec<Row> {
    records
        .iter()
        .take(limit)
        .map(|(id, record)| {
            let mut row = Row::new();
            row.insert(id_column.to_string(), id.clone());
            merge_properties(&mut row, &[id_column], record.properties.as_ref());
            row
        })
        .collect()
}

/// One row per ingested item-interaction event.
///
/// `timestamp` is `Event$sentAt`, "the timestamp (in Unix time) on the client side when
/// the event occurred" — the instant of the interaction itself, which is what an
/// Interactions record is keyed by in time, rather than when this emulator received it.
fn event_row(event: &ingestion::StoredEvent) -> Row {
    let mut row = Row::new();
    if let Some(user_id) = &event.user_id {
        row.insert(USER_ID_COLUMN.to_string(), user_id.clone());
    }
    if let Some(item_id) = &event.item_id {
        row.insert(ITEM_ID_COLUMN.to_string(), item_id.clone());
    }
    row.insert(
        TIMESTAMP_COLUMN.to_string(),
        event.sent_at.secs().to_string(),
    );
    row.insert(EVENT_TYPE_COLUMN.to_string(), event.event_type.clone());
    if let Some(event_value) = event.event_value {
        row.insert(EVENT_VALUE_COLUMN.to_string(), event_value.to_string());
    }
    merge_properties(
        &mut row,
        &[
            USER_ID_COLUMN,
            ITEM_ID_COLUMN,
            TIMESTAMP_COLUMN,
            EVENT_TYPE_COLUMN,
            EVENT_VALUE_COLUMN,
        ],
        event.properties.as_ref(),
    );
    row
}

/// One row per ingested action-interaction event, keyed in time by
/// `ActionInteraction$timestamp` for the same reason [`event_row`] uses `sentAt`.
fn action_interaction_row(interaction: &ingestion::StoredActionInteraction) -> Row {
    let mut row = Row::new();
    if let Some(user_id) = &interaction.user_id {
        row.insert(USER_ID_COLUMN.to_string(), user_id.clone());
    }
    row.insert(
        ACTION_ID_COLUMN.to_string(),
        interaction.action_id.clone(),
    );
    row.insert(
        TIMESTAMP_COLUMN.to_string(),
        interaction.timestamp.secs().to_string(),
    );
    row.insert(
        EVENT_TYPE_COLUMN.to_string(),
        interaction.event_type.clone(),
    );
    merge_properties(
        &mut row,
        &[
            USER_ID_COLUMN,
            ACTION_ID_COLUMN,
            TIMESTAMP_COLUMN,
            EVENT_TYPE_COLUMN,
        ],
        interaction.properties.as_ref(),
    );
    row
}
