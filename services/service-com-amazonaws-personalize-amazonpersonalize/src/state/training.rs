// SPDX-License-Identifier: CC0-1.0

//! What a solution version is TRAINED on: the catalog read out of a dataset group's own
//! records when `CreateSolutionVersion` runs, and which a campaign deploying that
//! version recommends from.
//!
//! # What is real here, and what Amazon Personalize does instead
//!
//! Amazon Personalize fits a learned model (User-Personalization, Personalized-Ranking,
//! SIMS, …) to the interactions, items and users of the dataset group and serves
//! inference from it. This emulator cannot fit those models, and it never invents a
//! catalog: everything below is READ from records that were actually put into the
//! dataset group —
//!
//! * the rows a `CreateDatasetImportJob` imported out of object storage
//!   ([`crate::state::DatasetRecord::imported`]), and
//! * the records and events the Amazon Personalize Events data plane ingested into the
//!   same datasets and the group's event trackers.
//!
//! What it fits instead is stated once, here, so every campaign agrees:
//!
//! * a catalog ENTRY is an item id (or an action id) that either the Items/Actions
//!   dataset describes or an interaction mentions;
//! * its POPULARITY is the number of interactions that mention it — the only signal
//!   these records carry about how likely it is to be chosen — and an entry nothing
//!   interacted with carries none;
//! * its METADATA columns are the other columns of its Items (or Actions) row, which is
//!   what a `metadataColumns` request and a filter expression read.
//!
//! The inference plane turns a popularity into a per-user prediction score of its own
//! (`scoring::Seed::relevance` in the runtime crate); nothing here claims to be a model.
//! Deterministic and derived from persisted rows: two solution versions trained on the
//! same records agree, and no campaign recommends an item nothing stored.

use std::collections::BTreeMap;

use ::service_com_amazonaws_personalizeevents_amazonpersonalizeevents::state as ingestion;
use ::service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state as runtime;

use crate::state::import::{column, Row, ACTION_ID_COLUMN, ITEM_ID_COLUMN, USER_ID_COLUMN};
use crate::state::ingested::properties_of;
use crate::state::{is_dataset_type, ComAmazonawsPersonalizeAmazonpersonalizeState};

/// The most catalog entries one solution version retains.
///
/// An EMULATOR bound on what one trained model holds, and therefore on what one campaign
/// registered in the inference registry keeps resident: a dataset may hold far more rows
/// ([`crate::state::import::MAX_DATASET_ROWS`]) than a developer host should keep a
/// per-campaign copy of. The entries kept are the most popular ones, so a bounded catalog
/// is the head of the same ordering a full one would report. How many catalogs the store
/// keeps at once is bounded in turn by
/// [`crate::state::pipeline::MAX_RETAINED_CATALOG_ENTRIES`], so the PRODUCT is capped and
/// not just each factor.
pub const MAX_CATALOG_ENTRIES: usize = 1_000;

/// The catalog a solution version was trained on.
#[derive(Debug, Clone, Default)]
pub struct TrainedCatalog {
    /// The items the model knows, most popular first.
    pub items: Vec<runtime::Item>,
    /// The actions the model knows, most popular first.
    pub actions: Vec<runtime::Action>,
    /// How many interactions the training data held, across items and actions. The
    /// training signal itself: a dataset group with none has nothing to fit a model to.
    pub interactions: usize,
    /// How many distinct users those interactions came from.
    pub users: usize,
}

impl TrainedCatalog {
    /// How many catalog entries this trained model holds, items and actions together:
    /// what one more trained version (and one more campaign deploying it) costs the
    /// store, measured against
    /// [`crate::state::pipeline::MAX_RETAINED_CATALOG_ENTRIES`].
    pub fn entries(&self) -> usize {
        self.items.len().saturating_add(self.actions.len())
    }
}

/// One entry under construction: its popularity and its metadata columns.
#[derive(Debug, Default)]
struct Entry {
    interactions: usize,
    attributes: BTreeMap<String, String>,
}

/// Train on everything the dataset group `dataset_group_arn` holds.
///
/// `ingested` is the Amazon Personalize Events store, when this engine carries it: the
/// records that data plane holds for the group's datasets and trackers are part of the
/// group's data exactly as the bulk-imported rows are ("how new bulk records influence
/// recommendations depends on the [...] recipe", `CreateDatasetImportJob` doc), so a
/// model trained without them would ignore data the caller streamed in.
///
/// A pure read of both stores: training derives a catalog, it changes no dataset.
pub fn train(
    store: &ComAmazonawsPersonalizeAmazonpersonalizeState,
    dataset_group_arn: &str,
    ingested: Option<&ingestion::ComAmazonawsPersonalizeeventsAmazonpersonalizeeventsState>,
) -> TrainedCatalog {
    let mut items: BTreeMap<String, Entry> = BTreeMap::new();
    let mut actions: BTreeMap<String, Entry> = BTreeMap::new();
    let mut users: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut interactions = 0_usize;

    for dataset in store
        .datasets
        .values()
        .filter(|dataset| dataset.dataset_group_arn == dataset_group_arn)
    {
        // The BULK rows an import job read into this dataset.
        if is_dataset_type(dataset, "Interactions") {
            for row in &dataset.imported {
                if let Some(item_id) = column(row, ITEM_ID_COLUMN) {
                    items.entry(item_id.to_string()).or_default().interactions += 1;
                    interactions += 1;
                }
                if let Some(user_id) = column(row, USER_ID_COLUMN) {
                    users.insert(user_id.to_string());
                }
            }
        } else if is_dataset_type(dataset, "Action_Interactions") {
            for row in &dataset.imported {
                if let Some(action_id) = column(row, ACTION_ID_COLUMN) {
                    actions.entry(action_id.to_string()).or_default().interactions += 1;
                    interactions += 1;
                }
                if let Some(user_id) = column(row, USER_ID_COLUMN) {
                    users.insert(user_id.to_string());
                }
            }
        } else if is_dataset_type(dataset, "Items") {
            for row in &dataset.imported {
                if let Some(item_id) = column(row, ITEM_ID_COLUMN) {
                    describe(items.entry(item_id.to_string()).or_default(), row, ITEM_ID_COLUMN);
                }
            }
        } else if is_dataset_type(dataset, "Actions") {
            for row in &dataset.imported {
                if let Some(action_id) = column(row, ACTION_ID_COLUMN) {
                    describe(
                        actions.entry(action_id.to_string()).or_default(),
                        row,
                        ACTION_ID_COLUMN,
                    );
                }
            }
        } else if is_dataset_type(dataset, "Users") {
            for row in &dataset.imported {
                if let Some(user_id) = column(row, USER_ID_COLUMN) {
                    users.insert(user_id.to_string());
                }
            }
        }

        // The records the INGESTION data plane holds for the same dataset.
        let Some(ingested) = ingested else { continue };
        let Some(streamed) = ingested.datasets.get(&dataset.dataset_arn) else {
            continue;
        };
        for (id, record) in &streamed.items {
            let entry = items.entry(id.clone()).or_default();
            for (name, value) in properties_of(record) {
                entry.attributes.insert(name, value);
            }
        }
        for (id, record) in &streamed.actions {
            let entry = actions.entry(id.clone()).or_default();
            for (name, value) in properties_of(record) {
                entry.attributes.insert(name, value);
            }
        }
        users.extend(streamed.users.keys().cloned());
    }

    // The events the group's own trackers ingested are interactions too.
    if let Some(ingested) = ingested {
        for tracker in ingested
            .event_trackers
            .values()
            .filter(|tracker| tracker.dataset_group_arn.as_deref() == Some(dataset_group_arn))
        {
            for event in &tracker.events {
                if let Some(item_id) = &event.item_id {
                    items.entry(item_id.clone()).or_default().interactions += 1;
                    interactions += 1;
                }
                if let Some(user_id) = &event.user_id {
                    users.insert(user_id.clone());
                }
            }
            for interaction in &tracker.action_interactions {
                actions
                    .entry(interaction.action_id.clone())
                    .or_default()
                    .interactions += 1;
                interactions += 1;
                if let Some(user_id) = &interaction.user_id {
                    users.insert(user_id.clone());
                }
            }
        }
    }

    TrainedCatalog {
        items: ranked(items)
            .into_iter()
            .map(|(id, entry)| runtime::Item {
                item_id: id,
                base_score: entry.interactions as f64,
                attributes: entry.attributes,
            })
            .collect(),
        actions: ranked(actions)
            .into_iter()
            .map(|(id, entry)| runtime::Action {
                action_id: id,
                base_score: entry.interactions as f64,
                attributes: entry.attributes,
            })
            .collect(),
        interactions,
        users: users.len(),
    }
}

/// Record an entry's metadata columns: every column of its own row except the one that
/// names it, which is the entry's id rather than a column of its metadata.
fn describe(entry: &mut Entry, row: &Row, id_column: &str) {
    for (name, value) in row {
        if name.eq_ignore_ascii_case(id_column) || value.is_empty() {
            continue;
        }
        entry.attributes.insert(name.clone(), value.clone());
    }
}

/// The entries of one catalog, most popular first and bounded by
/// [`MAX_CATALOG_ENTRIES`], with the id settling a tie so the catalog is the same on
/// every run.
fn ranked(entries: BTreeMap<String, Entry>) -> Vec<(String, Entry)> {
    let mut ranked: Vec<(String, Entry)> = entries.into_iter().collect();
    ranked.sort_by(|(left_id, left), (right_id, right)| {
        right
            .interactions
            .cmp(&left.interactions)
            .then_with(|| left_id.cmp(right_id))
    });
    ranked.truncate(MAX_CATALOG_ENTRIES);
    ranked
}
