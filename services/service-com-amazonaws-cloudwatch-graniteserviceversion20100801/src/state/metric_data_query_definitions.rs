// SPDX-License-Identifier: CC0-1.0

//! The METRIC DATA QUERY DEFINITIONS this store holds: the saved query each one is, the identity
//! it is keyed by, and the one write that creates or replaces one.
//!
//! One concern in one file: the stored definition ([`StoredMetricDataQueryDefinition`]), its
//! identity ([`MetricDataQueryDefinitionKey`]), the ARN a read reports it by
//! ([`metric_data_query_definition_arn`]), the PATH rules its writer judges a request's path by
//! ([`metric_data_query_definition_path_error`]) and the write itself
//! ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::put_metric_data_query_definition`]),
//! so the reader, the listing and the delete all see one record and one rule.
//!
//! Three rules live here, and every operation on a definition goes through them:
//!
//! * a definition is a REGIONAL resource — "The path must be unique in your account and Region"
//!   and "You can have up to 1,000 metric data query definitions in each Region of your account"
//!   (`PutMetricDataQueryDefinition`), and its ARN carries the Region — so the Region is part of
//!   the key a definition is held under and not merely part of what it reports;
//! * a WRITE that carries no `Id` CREATES a definition and mints its identifier
//!   ([`minted_metric_data_query_definition_id`]), while one that carries an `Id` REPLACES the
//!   path, the expression and the type of the definition that identifier names and keeps the
//!   instant it was created at: "An update replaces the path, expression, and type, and keeps
//!   the creation time.";
//! * the PATH is the definition's name and is unique in the Region, so it is what the listing is
//!   ordered by and what a path already taken by another definition is refused for.

use super::{to_timestamp, ComAmazonawsCloudwatchGraniteserviceversion20100801State};
use crate::ops::types::{
    GetMetricDataQueryDefinitionOutput, MetricDataQueryDefinitionSummary, Tag,
};

/// The identity of one metric data query definition: the Region it lives in and its id.
///
/// Keyed by Region because the resource is regional — its ARN carries the Region
/// (`arn:aws:cloudwatch:Region:account-id:metric-query-definition/Id`,
/// `PutMetricDataQueryDefinition`) and both the path's uniqueness and the number of definitions
/// allowed are stated per Region — so the same id reached through another Region's endpoint names
/// no definition of this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MetricDataQueryDefinitionKey {
    /// The Region the definition lives in, as the request that wrote it was signed for.
    pub region: String,
    /// The definition's id, minted by the write that created it.
    pub id: String,
}

/// The key one Region's definition of this id is held under.
pub fn metric_data_query_definition_key(region: &str, id: &str) -> MetricDataQueryDefinitionKey {
    MetricDataQueryDefinitionKey {
        region: region.to_string(),
        id: id.to_string(),
    }
}

/// A metric data query definition's ARN:
/// `arn:aws:cloudwatch:<region>:<account>:metric-query-definition/<Id>`, the form
/// `PutMetricDataQueryDefinition` documents the resource by ("The resource ARN has the format
/// arn:aws:cloudwatch:Region:account-id:metric-query-definition/Id").
pub fn metric_data_query_definition_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "cloudwatch",
        region,
        &format!("metric-query-definition/{id}"),
    )
}

/// The `ResourceType` a not-found about a metric data query definition reports, the kind of
/// resource the request named (`ResourceNotFoundException$ResourceType`) — the resource's own
/// name in this model, as every other not-found of this class in this service names its kind.
///
/// Shared by the reader and the writer, so both name the kind the same way.
pub const METRIC_DATA_QUERY_DEFINITION_RESOURCE_TYPE: &str = "MetricDataQueryDefinition";

/// How many metric data query definitions one Region of the account holds: "You can have up to
/// 1,000 metric data query definitions in each Region of your account. A request that would
/// create more fails with a LimitExceededException." (`PutMetricDataQueryDefinition`).
pub const MAX_METRIC_DATA_QUERY_DEFINITIONS: usize = 1_000;

/// The largest number of metric data query definitions this in-memory store holds across every
/// Region.
///
/// A capacity bound, not an AWS quota, and a separate one from the per-Region quota above: that
/// quota is stated per Region while a caller can sign a request for any Region, so the records —
/// which live in this process's memory — have to be bounded here too. A create that would pass it
/// is answered through the same modelled quota error; updating a definition that already exists
/// adds none, so it is unaffected.
pub const MAX_METRIC_DATA_QUERY_DEFINITIONS_HELD: usize = 10_000;

/// The longest path a definition carries: "It can be at most 1,015 bytes when encoded as UTF-8."
/// (`PutMetricDataQueryDefinitionInput$Path`) — a bound the model cannot express, since
/// `DefinitionPath`'s own `@length` counts CHARACTERS (1..1024) rather than encoded bytes.
pub const MAX_DEFINITION_PATH_BYTES: usize = 1_015;

/// The folder separator a path organises definitions with: "Use slashes (/) to organize
/// definitions into folders, for example team/web/latency."
/// (`PutMetricDataQueryDefinitionInput$Path`).
const PATH_SEPARATOR: char = '/';

/// Two separators with nothing between them, which is what an EMPTY FOLDER NAME in a path looks
/// like: "It can't contain an empty folder name (//)."
/// (`PutMetricDataQueryDefinitionInput$Path`).
const EMPTY_FOLDER_NAME: &str = "//";

/// The id a write that CREATES a definition mints for it.
///
/// "CloudWatch generates the ID and returns it in the response."
/// (`PutMetricDataQueryDefinitionInput$Id`) — an opaque value the SERVICE mints, so nothing in it
/// comes from the request: two definitions created with identical paths and expressions carry
/// different ids, and a definition deleted and created again never reproduces the id it had.
///
/// A uuid, which is inside the modelled `DefinitionId` (length 1..64, pattern
/// `^[a-zA-Z0-9-]{1,64}$`), rendered by the workspace's ONE rendering of 16 opaque bytes as a
/// version-4 uuid over the mixing every identity this service mints draws from
/// ([`super::alarm_reads::minted_bytes`]) — so neither the randomness nor the layout is spelled
/// out a second time here.
#[must_use]
pub fn minted_metric_data_query_definition_id() -> String {
    ::service_common::uuid_from_random_bytes(super::alarm_reads::minted_bytes())
}

/// Why `path` is no path a definition can carry, or `None` when it is one.
///
/// The three requirements the member states that its modelled `pattern` and `@length` cannot:
/// "It can't start or end with a slash. It can't contain an empty folder name (//). It can be at
/// most 1,015 bytes when encoded as UTF-8." (`PutMetricDataQueryDefinitionInput$Path`). The
/// fourth — that the path be unique in the account and Region — is a property of the STORE
/// rather than of the value, so it is judged by the write itself
/// ([`ComAmazonawsCloudwatchGraniteserviceversion20100801State::put_metric_data_query_definition`])
/// and refused as the conflict it is.
///
/// Each sentence names the member at fault and states the requirement it broke, and none of them
/// echoes the path: the member runs to 1,015 bytes while the refusal's own message member is
/// modelled far shorter, so the value is what the caller already holds rather than part of the
/// sentence.
#[must_use]
pub fn metric_data_query_definition_path_error(path: &str) -> Option<String> {
    if path.starts_with(PATH_SEPARATOR) || path.ends_with(PATH_SEPARATOR) {
        return Some("Path can't start or end with a slash.".to_string());
    }
    if path.contains(EMPTY_FOLDER_NAME) {
        return Some("Path can't contain an empty folder name.".to_string());
    }
    (path.len() > MAX_DEFINITION_PATH_BYTES).then(|| {
        format!(
            "Path can be at most {MAX_DEFINITION_PATH_BYTES} bytes when encoded as UTF-8."
        )
    })
}

/// A stored metric data query definition: the shape its reader returns, plus the tags the write
/// that created it accepted (no member of that shape carries them).
///
/// Held as [`GetMetricDataQueryDefinitionOutput`] itself rather than as a parallel record, so
/// every member `PutMetricDataQueryDefinition` accepts round-trips through
/// `GetMetricDataQueryDefinition` without a field-by-field copy that can silently drop one, and
/// the listing's summary is a projection of the same record.
#[derive(Debug, Clone)]
pub struct StoredMetricDataQueryDefinition {
    /// The definition as its reader reports it.
    pub definition: GetMetricDataQueryDefinitionOutput,
    /// The tags the definition carries: "Tags are applied only when the request creates a
    /// definition, that is, when you omit Id." (`PutMetricDataQueryDefinitionInput$Tags`), so an
    /// update leaves this set exactly as the create left it.
    pub tags: Vec<Tag>,
}

impl StoredMetricDataQueryDefinition {
    /// The definition as a READ of it reports it: every member as the write left it.
    #[must_use]
    pub fn reported(&self) -> GetMetricDataQueryDefinitionOutput {
        self.definition.clone()
    }

    /// The listing of this definition.
    ///
    /// `MetricDataQueryDefinitionSummary` is the SUBSET of a definition the listing carries —
    /// "The results are summaries that don't include the expression. To get the full definition,
    /// call GetMetricDataQueryDefinition with its ID." (`ListMetricDataQueryDefinitions`) — so
    /// each member it models is copied from the stored record and the expression is left to the
    /// reader that does model it.
    #[must_use]
    pub fn summary(&self) -> MetricDataQueryDefinitionSummary {
        MetricDataQueryDefinitionSummary {
            id: self.definition.id.clone(),
            path: self.definition.path.clone(),
            r#type: self.definition.r#type.clone(),
            updated_at: Some(self.definition.updated_at),
        }
    }
}

/// What one `PutMetricDataQueryDefinition` asks to be written: the definition it names, or none
/// when it asks for a definition to be created.
#[derive(Debug, Clone, Copy)]
pub struct MetricDataQueryDefinitionWrite<'a> {
    /// The id of the definition to update, or `None` to create one: "The ID of the metric data
    /// query definition to update. Omit this parameter to create a definition."
    /// (`PutMetricDataQueryDefinitionInput$Id`).
    pub id: Option<&'a str>,
    /// The path the definition takes.
    pub path: &'a str,
    /// The expression the definition saves.
    pub expression: &'a str,
    /// The language the expression is in.
    pub query_type: &'a str,
    /// The tags a CREATE applies; an update applies none.
    pub tags: &'a [Tag],
}

/// Why a write of a metric data query definition could not be made. The caller renders each
/// through the error its own model declares for that class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricDataQueryDefinitionRefusal {
    /// The write named an `Id` this Region holds no definition under: there is nothing to
    /// update.
    NoSuchDefinition,
    /// The path the write asked for is already used by ANOTHER definition of this Region.
    PathAlreadyUsed,
    /// The Region already holds every definition it can hold
    /// ([`MAX_METRIC_DATA_QUERY_DEFINITIONS`]), so no further one can be created.
    TooManyDefinitions,
    /// This store holds every definition it can hold across every Region
    /// ([`MAX_METRIC_DATA_QUERY_DEFINITIONS_HELD`]), so no further one can be created.
    StoreAtCapacity,
}

impl ComAmazonawsCloudwatchGraniteserviceversion20100801State {
    /// The definition the id `id` names in `region`, or `None` when this store holds none.
    #[must_use]
    pub fn metric_data_query_definition(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&StoredMetricDataQueryDefinition> {
        self.metric_data_query_definitions
            .get(&metric_data_query_definition_key(region, id))
    }

    /// Every definition of `region`, ordered by PATH — the definition's name, unique in the
    /// Region, so the order is total and a listing's cursor can name the definition a page
    /// starts at.
    #[must_use]
    pub fn metric_data_query_definitions_of(
        &self,
        region: &str,
    ) -> Vec<&StoredMetricDataQueryDefinition> {
        let mut selected: Vec<&StoredMetricDataQueryDefinition> = self
            .metric_data_query_definitions
            .iter()
            .filter(|(key, _)| key.region == region)
            .map(|(_, stored)| stored)
            .collect();
        selected.sort_unstable_by(|left, right| left.definition.path.cmp(&right.definition.path));
        selected
    }

    /// Create or replace a metric data query definition of `region`, answering the definition as
    /// a read of it reports it.
    ///
    /// The one write of this resource, so a create and an update cannot disagree about what a
    /// definition is: a write carrying no `Id` mints one and stamps both instants, a write
    /// carrying an `Id` replaces the path, the expression and the type of that definition,
    /// stamps `UpdatedAt` and keeps `CreatedAt` and the tags — "An update replaces the path,
    /// expression, and type, and keeps the creation time."
    /// (`PutMetricDataQueryDefinition`). A path another definition of the Region already uses is
    /// refused whichever of the two the write is, since the path is unique in the Region.
    pub fn put_metric_data_query_definition(
        &mut self,
        region: &str,
        write: MetricDataQueryDefinitionWrite<'_>,
        now_secs: f64,
    ) -> Result<GetMetricDataQueryDefinitionOutput, MetricDataQueryDefinitionRefusal> {
        // "The path must be unique in your account and Region. If another definition already
        // uses the path, the request fails with a ConflictException."
        // (`PutMetricDataQueryDefinition`) — ANOTHER definition, so a write that leaves a
        // definition's own path where it is conflicts with nothing.
        let taken_by_another = self
            .metric_data_query_definitions
            .iter()
            .any(|(key, stored)| {
                key.region == region
                    && stored.definition.path == write.path
                    && write.id != Some(key.id.as_str())
            });
        if taken_by_another {
            return Err(MetricDataQueryDefinitionRefusal::PathAlreadyUsed);
        }

        let now = to_timestamp(now_secs);
        if let Some(id) = write.id {
            let stored = self
                .metric_data_query_definitions
                .get_mut(&metric_data_query_definition_key(region, id))
                .ok_or(MetricDataQueryDefinitionRefusal::NoSuchDefinition)?;
            stored.definition.path = write.path.to_string();
            stored.definition.expression = write.expression.to_string();
            stored.definition.r#type = write.query_type.to_string();
            stored.definition.updated_at = now;
            return Ok(stored.definition.clone());
        }

        if self
            .metric_data_query_definitions
            .keys()
            .filter(|key| key.region == region)
            .count()
            >= MAX_METRIC_DATA_QUERY_DEFINITIONS
        {
            return Err(MetricDataQueryDefinitionRefusal::TooManyDefinitions);
        }
        if self.metric_data_query_definitions.len() >= MAX_METRIC_DATA_QUERY_DEFINITIONS_HELD {
            return Err(MetricDataQueryDefinitionRefusal::StoreAtCapacity);
        }

        // The minted id names a definition that does not exist yet. Minted again in the
        // vanishingly unlikely case that it names one already, so a create can never replace a
        // definition a caller never named.
        let mut key = metric_data_query_definition_key(
            region,
            &minted_metric_data_query_definition_id(),
        );
        while self.metric_data_query_definitions.contains_key(&key) {
            key = metric_data_query_definition_key(
                region,
                &minted_metric_data_query_definition_id(),
            );
        }
        let definition = GetMetricDataQueryDefinitionOutput {
            id: key.id.clone(),
            arn: metric_data_query_definition_arn(region, &key.id),
            path: write.path.to_string(),
            expression: write.expression.to_string(),
            r#type: write.query_type.to_string(),
            created_at: now,
            updated_at: now,
        };
        self.metric_data_query_definitions.insert(
            key,
            StoredMetricDataQueryDefinition {
                definition: definition.clone(),
                tags: write.tags.to_vec(),
            },
        );
        Ok(definition)
    }

    /// Remove the definition `id` of `region`, answering whether one was there to remove.
    ///
    /// "This operation is idempotent. If the definition doesn't exist, the operation succeeds
    /// without returning an error." (`DeleteMetricDataQueryDefinition`) — so the answer is for
    /// the caller's own bookkeeping, not a refusal: both outcomes leave the Region holding no
    /// definition under that id.
    pub fn delete_metric_data_query_definition(&mut self, region: &str, id: &str) -> bool {
        self.metric_data_query_definitions
            .remove(&metric_data_query_definition_key(region, id))
            .is_some()
    }
}
