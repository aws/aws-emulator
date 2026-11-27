// SPDX-License-Identifier: CC0-1.0

//! The ENGINE STATUS this data plane reports: the record it is read from, the
//! configuration of the cluster it answers for, and the forms `GetEngineStatus` renders.
//!
//! The /status endpoint reports on an ENGINE, and an engine is something the control plane
//! provisioned: the version it runs, the query-language versions that version carries, and
//! whether a feature a cluster can be created with is on are all facts of the DB cluster
//! this plane is addressed at, not constants of the emulator. So the record below holds what
//! belongs to this engine PROCESS — when it started, what it is rolling back, the engine
//! defaults of every setting a cluster does not carry — and [`EngineStatusRecord::reported`]
//! layers the provisioned cluster's own configuration over it, read out of the shared
//! directory the Neptune control plane records a created cluster in
//! ([`::service_common::graph_db_clusters`]). With no cluster provisioned at all the engine
//! answers its own defaults rather than inventing one.
//!
//! Two forms the endpoint renders are stated here too, because both are facts of /status
//! rather than of any one member:
//!
//! * a feature's entry under `features` is the DOCUMENT the engine reports for it
//!   ([`FeatureReport`]) — the status string itself for a feature whose status is all it
//!   reports, and a document of its own for a feature that reports one;
//! * a lab-mode parameter's entry under `labMode` is the value its own KIND states
//!   ([`LabModeParameter`]) — the on/off word for a switch, and a quantity for a parameter
//!   whose name states one;
//! * an instant is rendered the way /status renders one ([`engine_timestamp`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

pub use ::service_common::graph_db_clusters::{GraphClusterConfiguration, GraphDbClusters};

/// `GetEngineStatusOutput$status` for an instance that is not experiencing problems:
/// the member's own documentation says "Set to `healthy` if the instance is not
/// experiencing problems".
pub const ENGINE_STATUS_HEALTHY: &str = "healthy";

/// `GetEngineStatusOutput$role` for the primary instance: the member's documentation
/// says "Set to `reader` if the instance is a read-replica, or to `writer` if the
/// instance is the primary instance". This emulator serves one instance, and it accepts
/// writes, so it is the primary.
pub const ENGINE_ROLE_WRITER: &str = "writer";

/// `GetEngineStatusOutput$dfeQueryEngine`'s documented default — the member's own
/// documentation names `viaQueryHint` "(the default)".
pub const DFE_QUERY_ENGINE_DEFAULT: &str = "viaQueryHint";

/// The engine version this plane reports while NO cluster has been provisioned — the
/// Neptune engine line this emulator's graph engine serves, in the four-part form the
/// control plane states a version in.
///
/// A provisioned cluster replaces it: what /status reports is the version the cluster runs
/// ([`EngineStatusRecord::reported`]), so this value is only what an engine nobody has
/// created a cluster on answers.
pub const DEFAULT_DB_ENGINE_VERSION: &str = "1.4.8.0";

/// The release suffix a /status `dbEngineVersion` carries past the four-part version the
/// control plane reports, for a version whose published release is its FIRST: the control
/// plane states the version a cluster was created with (`1.4.8.0`) and the engine states the
/// RELEASE of it that is running (`1.4.8.0.R1`).
///
/// Which build a version's published release carries comes from the one shared table of this
/// engine's published releases (`::service_common::neptune_engine_releases`), which the
/// control plane's version description reads too, so the two planes cannot name different
/// releases of one version; see [`status_engine_version`].
pub const ENGINE_RELEASE_SUFFIX: &str = ::service_common::neptune_engine_releases::FIRST_BUILD;

/// The SPARQL version `GetEngineStatusOutput$sparql` reports. One version across the engine
/// lines: SPARQL 1.1 is the W3C recommendation Neptune implements.
pub const SPARQL_LANGUAGE_VERSION: &str = "sparql-1.1";

/// The openCypher version `GetEngineStatusOutput$opencypher` reports.
pub const OPENCYPHER_LANGUAGE_VERSION: &str = "Neptune-9.0.20190305-1.0";

/// The Apache TinkerPop version each Neptune ENGINE LINE ships, by the line's major and
/// minor version, earliest line first.
///
/// Source: the Gremlin version each Neptune engine release states it carries — the 1.3 line
/// ships TinkerPop 3.6.2 and the 1.4 line ships 3.7.1. The line is what moves it, not the
/// patch: every release of one line carries the same Gremlin version.
const TINKERPOP_BY_ENGINE_LINE: [((u32, u32), &str); 2] =
    [((1, 3), TINKERPOP_ON_EARLIEST_LINE), ((1, 4), "tinkerpop-3.7.1")];

/// The Gremlin version of the EARLIEST line [`TINKERPOP_BY_ENGINE_LINE`] names, which is
/// also what a version no line of the catalog covers reports.
const TINKERPOP_ON_EARLIEST_LINE: &str = "tinkerpop-3.6.2";

/// The cluster query timeout, in milliseconds, reported under `settings` — the member's
/// own documentation names `clusterQueryTimeoutInMs` as what `settings` carries, and
/// 120000 ms is the documented default of the `neptune_query_timeout` DB cluster
/// parameter it reflects.
pub const CLUSTER_QUERY_TIMEOUT_MS: &str = "120000";

/// The settings key [`CLUSTER_QUERY_TIMEOUT_MS`] is reported under.
pub const CLUSTER_QUERY_TIMEOUT_SETTING: &str = "clusterQueryTimeoutInMs";

/// The slow-query threshold reported under `settings`, in milliseconds: the documented
/// default of the `neptune_slow_query_log_threshold` DB cluster parameter it reflects, which
/// is the duration past which a query counts as slow.
pub const SLOW_QUERY_LOGS_THRESHOLD_MS: &str = "5000";

/// The settings key [`SLOW_QUERY_LOGS_THRESHOLD_MS`] is reported under.
pub const SLOW_QUERY_LOGS_THRESHOLD_SETTING: &str = "SlowQueryLogsThreshold";

/// Whether the engine validates a query's requested timeout strictly, reported under
/// `settings`: the documented default of the `neptune_strict_timeout_validation` DB cluster
/// parameter it reflects, which is on.
pub const STRICT_TIMEOUT_VALIDATION: &str = "true";

/// The settings key [`STRICT_TIMEOUT_VALIDATION`] is reported under.
pub const STRICT_TIMEOUT_VALIDATION_SETTING: &str = "StrictTimeoutValidation";

/// The lab-mode parameter naming the size, in MB as its own name states, a graph's table
/// space extends itself by. `labMode` is a `StringValuedMap`
/// (`smithy_query(query="shape", shape="GetEngineStatusOutput")`), so what it reports for
/// this parameter is a string like every other lab-mode value — but a string that states a
/// SIZE, not one of the on/off words a switch-valued parameter states
/// ([`LabModeParameter`]).
pub const TABLE_SPACE_AUTO_EXTEND_SETTING: &str = "TableSpaceAutoExtendSizeInMB";

/// The lab-mode parameter whose status is whether the engine maintains an object index.
pub const OBJECT_INDEX_SETTING: &str = "ObjectIndex";

/// The lab-mode parameter whose status is whether the engine detects read/write conflicts.
pub const READ_WRITE_CONFLICT_DETECTION_SETTING: &str = "ReadWriteConflictDetection";

/// The auto-extend size [`TABLE_SPACE_AUTO_EXTEND_SETTING`] is in force with on this engine:
/// none. This emulator's graph is held in memory and extends its table space by no
/// configured number of megabytes, and a size-valued parameter with no size in force reports
/// no size at all.
pub const TABLE_SPACE_AUTO_EXTEND_SIZE_MB: Option<u64> = None;

/// A feature, lab-mode parameter or setting that is ON.
pub const FEATURE_ENABLED: &str = "enabled";

/// A feature, lab-mode parameter or setting that is OFF.
pub const FEATURE_DISABLED: &str = "disabled";

/// The feature whose status is whether the cluster accepts IAM database authentication.
pub const IAM_AUTHENTICATION_FEATURE: &str = "IAMAuthentication";

/// The feature whose status is whether the cluster collects an audit log.
pub const AUDIT_LOG_FEATURE: &str = "AuditLog";

/// The feature whose status is whether the cluster collects a slow-query log.
pub const SLOW_QUERY_LOGS_FEATURE: &str = "SlowQueryLogs";

/// The feature whose status is whether the cluster serves a change stream.
pub const STREAMS_FEATURE: &str = "Streams";

/// The feature whose status is whether edge ids the engine generates are returned inline
/// with the edge.
pub const INLINE_SERVER_GENERATED_EDGE_ID_FEATURE: &str = "InlineServerGeneratedEdgeId";

/// The feature whose entry carries a DOCUMENT of its own rather than a bare status: the DFE
/// result cache reports a record, not a word.
pub const RESULT_CACHE_FEATURE: &str = "ResultCache";

/// The member a feature's own document states its status under.
pub const FEATURE_STATUS_MEMBER: &str = "status";

/// What `features` reports for ONE feature — the map's value is a `Document`
/// (`DocumentValuedMap`: key `String`, value `Document`), and the engine uses that freedom:
/// most features report their status as the bare string it is, and a feature that has more
/// than a status to report answers a document of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureReport {
    /// The feature's status, reported as the string itself.
    Status(String),
    /// The document this feature reports, by member name.
    Document(BTreeMap<String, String>),
}

impl FeatureReport {
    /// A feature that reports its status as a bare string: on when `enabled`.
    pub fn status(enabled: bool) -> Self {
        Self::Status(feature_status(enabled).to_string())
    }

    /// A feature that reports a document whose one member is its status.
    pub fn status_document(enabled: bool) -> Self {
        Self::Document(BTreeMap::from([(
            FEATURE_STATUS_MEMBER.to_string(),
            feature_status(enabled).to_string(),
        )]))
    }

    /// This report as the `Document` the map's value is on the wire.
    pub fn document(&self) -> ::aws_smithy_types::Document {
        match self {
            Self::Status(status) => ::aws_smithy_types::Document::String(status.clone()),
            Self::Document(members) => ::aws_smithy_types::Document::Object(
                members
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            ::aws_smithy_types::Document::String(value.clone()),
                        )
                    })
                    .collect(),
            ),
        }
    }
}

/// What ONE lab-mode parameter is, and so what /status reports for it.
///
/// `labMode` is a map of strings, and the string a parameter carries is in the vocabulary
/// that parameter's own kind has: a SWITCH carries the on/off word every other switch of the
/// engine carries, and a parameter whose name states a quantity carries that quantity. The
/// two vocabularies are not interchangeable — a size-valued parameter never reports an on/off
/// word, and reports no value at all while no size is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabModeParameter {
    /// A parameter that is a switch: whether it is on is all it reports.
    Switch(bool),
    /// A parameter that names a SIZE in the unit its own name states, and the number of those
    /// units in force — `None` while the engine runs with no size configured for it.
    Size(Option<u64>),
}

impl LabModeParameter {
    /// What `labMode` reports for this parameter: the on/off word for a switch, the number of
    /// units for a size that is in force, and the empty value for a size that is not.
    pub fn reported(&self) -> String {
        match self {
            Self::Switch(on) => feature_status(*on).to_string(),
            Self::Size(units) => units.map(|units| units.to_string()).unwrap_or_default(),
        }
    }
}

/// The word a status member carries for a switch that is on or off.
pub fn feature_status(enabled: bool) -> &'static str {
    if enabled {
        FEATURE_ENABLED
    } else {
        FEATURE_DISABLED
    }
}

/// The engine state `GetEngineStatus` reports: one record, built at state init.
#[derive(Debug, Clone)]
pub struct EngineStatusRecord {
    /// `healthy` for an instance with no problem to report.
    pub status: String,
    /// When this emulator's engine process started. Fixed at state init: a later read
    /// reports this instant, never a fresh clock reading.
    pub start_time: SystemTime,
    /// The engine version reported, in the form /status states one: the provisioned
    /// cluster's version where there is one, else [`DEFAULT_DB_ENGINE_VERSION`].
    pub db_engine_version: String,
    pub role: String,
    pub dfe_query_engine: String,
    pub gremlin_version: String,
    pub sparql_version: String,
    pub opencypher_version: String,
    /// The Lab Mode parameters this engine carries, by parameter name — each held as what it
    /// IS, so what /status reports for it is rendered from its own kind.
    pub lab_mode: BTreeMap<String, LabModeParameter>,
    /// How many transactions are being rolled back right now.
    pub rolling_back_trx_count: i32,
    /// When the earliest transaction being rolled back started. `None` while nothing is
    /// rolling back, which is what keeps the member off the wire.
    pub rolling_back_trx_earliest_start_time: Option<SystemTime>,
    /// What each optional engine feature reports, by feature name.
    pub features: BTreeMap<String, FeatureReport>,
    /// The cluster settings this engine runs with, by setting name.
    pub settings: BTreeMap<String, String>,
}

impl Default for EngineStatusRecord {
    fn default() -> Self {
        Self {
            status: ENGINE_STATUS_HEALTHY.to_string(),
            start_time: SystemTime::now(),
            db_engine_version: status_engine_version(DEFAULT_DB_ENGINE_VERSION),
            role: ENGINE_ROLE_WRITER.to_string(),
            dfe_query_engine: DFE_QUERY_ENGINE_DEFAULT.to_string(),
            gremlin_version: gremlin_language_version(DEFAULT_DB_ENGINE_VERSION).to_string(),
            sparql_version: SPARQL_LANGUAGE_VERSION.to_string(),
            opencypher_version: OPENCYPHER_LANGUAGE_VERSION.to_string(),
            // Neptune's engine-level Lab Mode parameters at their defaults: the two switches
            // are off and on respectively unless a cluster moves them, and the table-space
            // auto-extend size is the size this engine runs with, which is none.
            lab_mode: BTreeMap::from([
                (
                    OBJECT_INDEX_SETTING.to_string(),
                    LabModeParameter::Switch(false),
                ),
                (
                    READ_WRITE_CONFLICT_DETECTION_SETTING.to_string(),
                    LabModeParameter::Switch(true),
                ),
                (
                    TABLE_SPACE_AUTO_EXTEND_SETTING.to_string(),
                    LabModeParameter::Size(TABLE_SPACE_AUTO_EXTEND_SIZE_MB),
                ),
            ]),
            // No transaction is rolling back in a freshly started engine, so the
            // earliest-rollback instant has nothing to report.
            rolling_back_trx_count: 0,
            rolling_back_trx_earliest_start_time: None,
            // Every optional engine feature is off until a cluster turns it on, and each
            // entry below says something true of this emulator: there is no DFE result
            // cache, the stream endpoints are not implemented, no log is collected, no
            // cluster has asked for IAM authentication, and edge ids are not inlined. The
            // three a cluster's own configuration settles are replaced by
            // [`EngineStatusRecord::reported`].
            features: BTreeMap::from([
                (AUDIT_LOG_FEATURE.to_string(), FeatureReport::status(false)),
                (
                    IAM_AUTHENTICATION_FEATURE.to_string(),
                    FeatureReport::status(false),
                ),
                (
                    INLINE_SERVER_GENERATED_EDGE_ID_FEATURE.to_string(),
                    FeatureReport::status(false),
                ),
                (
                    RESULT_CACHE_FEATURE.to_string(),
                    FeatureReport::status_document(false),
                ),
                (
                    SLOW_QUERY_LOGS_FEATURE.to_string(),
                    FeatureReport::status(false),
                ),
                (STREAMS_FEATURE.to_string(), FeatureReport::status(false)),
            ]),
            settings: BTreeMap::from([
                (
                    CLUSTER_QUERY_TIMEOUT_SETTING.to_string(),
                    CLUSTER_QUERY_TIMEOUT_MS.to_string(),
                ),
                (
                    SLOW_QUERY_LOGS_THRESHOLD_SETTING.to_string(),
                    SLOW_QUERY_LOGS_THRESHOLD_MS.to_string(),
                ),
                (
                    STRICT_TIMEOUT_VALIDATION_SETTING.to_string(),
                    STRICT_TIMEOUT_VALIDATION.to_string(),
                ),
            ]),
        }
    }
}

impl EngineStatusRecord {
    /// This record as /status REPORTS it for the cluster this plane answers for.
    ///
    /// What the CLUSTER settles is taken from `cluster`: the engine version it was
    /// provisioned with, the query-language versions that engine carries, and the status of
    /// every feature a create turns on. Everything else is this engine process's own and is
    /// reported unchanged — and with no cluster provisioned at all (`None`) the record is
    /// reported exactly as it stands, because there is no configuration to read.
    ///
    /// A pure reading: it borrows the record and hands back a value, so reporting the status
    /// changes nothing a later caller sees.
    pub fn reported(&self, cluster: Option<&GraphClusterConfiguration>) -> Self {
        let mut reported = self.clone();
        let Some(cluster) = cluster else {
            return reported;
        };
        reported.db_engine_version = status_engine_version(&cluster.engine_version);
        reported.gremlin_version = gremlin_language_version(&cluster.engine_version).to_string();
        for (feature, enabled) in [
            (
                IAM_AUTHENTICATION_FEATURE,
                cluster.iam_database_authentication_enabled,
            ),
            (AUDIT_LOG_FEATURE, cluster.audit_log_enabled),
            (SLOW_QUERY_LOGS_FEATURE, cluster.slow_query_log_enabled),
        ] {
            reported
                .features
                .insert(feature.to_string(), FeatureReport::status(enabled));
        }
        reported
    }
}

/// `engine_version` as /status states it: the control plane's four-part version with the
/// build of the release the engine publishes for that version after it
/// (`::service_common::neptune_engine_releases::release`, whose answer for a version
/// published with no later build is [`ENGINE_RELEASE_SUFFIX`]).
///
/// A value that is not four numeric parts is reported as it stands: it already carries a
/// release suffix, or it is not a version this engine can name a release of, and either way
/// appending a second suffix would report a version that does not exist.
pub fn status_engine_version(engine_version: &str) -> String {
    ::service_common::neptune_engine_releases::release(engine_version)
}

/// The Apache TinkerPop version the engine `engine_version` names carries.
///
/// The catalog is keyed by the engine LINE ([`TINKERPOP_BY_ENGINE_LINE`]). A line the
/// catalog does not name reports the newest line it names that is not later than the asked
/// one — a patch line the catalog has not caught up with carries its line's version — and a
/// version earlier than every line it names, or one whose line cannot be read at all,
/// reports the earliest.
pub fn gremlin_language_version(engine_version: &str) -> &'static str {
    let line = engine_line(engine_version);
    let mut carried = TINKERPOP_ON_EARLIEST_LINE;
    for (catalogued, version) in TINKERPOP_BY_ENGINE_LINE {
        if line.is_some_and(|asked| asked >= catalogued) {
            carried = version;
        }
    }
    carried
}

/// The engine LINE `engine_version` names — its major and minor version — or `None` when the
/// value does not start with two numeric parts.
fn engine_line(engine_version: &str) -> Option<(u32, u32)> {
    let mut parts = engine_version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// What the cluster this plane answers for was provisioned with, or `None` when no cluster
/// has been.
///
/// The directory lives in the SAME shared engine as this service's own store, and the
/// Neptune control plane records a created cluster there; the one reading of it is the shared
/// handle ([`GraphDbClusters`]), so this plane spells no engine downcast of its own. One
/// emulator process answers every cluster's data plane at its own one address, so the cluster
/// a /status call is addressing is the one provisioned most recently
/// ([`GraphDbClusters::answering_cluster`]), and an engine that vends no directory at all is
/// an engine nobody created a cluster on — the engine's own defaults are reported for it.
///
/// LOCK ORDER: this plane takes its own store and the directory one at a time and never
/// holds both, so no order between them can invert.
pub fn provisioned_cluster<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<GraphClusterConfiguration> {
    GraphDbClusters::of(engine)?.answering_cluster()
}

/// An instant as the engine-status timestamps report it: `EEE MMM dd HH:mm:ss UTC yyyy` —
/// the weekday, the month, the zero-padded day, the clock time, the zone and the year, which
/// is the form /status states `startTime` in.
///
/// The calendar is a library's, not this crate's: `time` resolves the instant into the
/// weekday, month and date fields the form names, so no date arithmetic lives here. An
/// instant the formatter cannot render — one outside the years it can print — reports as the
/// empty string rather than failing a status read.
pub fn engine_timestamp(instant: SystemTime) -> String {
    let rendered = ::time::OffsetDateTime::from(instant).format(::time::macros::format_description!(
        "[weekday repr:short] [month repr:short] [day] [hour]:[minute]:[second] UTC [year]"
    ));
    debug_assert!(
        rendered.is_ok(),
        "an instant this emulator stores must be printable: {rendered:?}"
    );
    rendered.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// RULE: the engine version /status reports is the provisioned cluster's own version in
    /// the release form, and the Gremlin version is the one that engine line carries.
    ///
    /// The rows span what the rule generalises over: the engine lines the catalog names, a
    /// later line it has not caught up with, an earlier one whose published release is a
    /// later BUILD of that version, a version that already carries a release suffix, and a
    /// value that is no version at all.
    #[test]
    fn the_reported_versions_follow_the_provisioned_engine_version() {
        let rows: &[(&str, &str, &str, &str)] = &[
            ("the 1.4 line", "1.4.8.0", "1.4.8.0.R1", "tinkerpop-3.7.1"),
            (
                "another release of the 1.4 line",
                "1.4.1.0",
                "1.4.1.0.R1",
                "tinkerpop-3.7.1",
            ),
            ("the 1.3 line", "1.3.2.1", "1.3.2.1.R1", "tinkerpop-3.6.2"),
            (
                "a line past the catalog",
                "1.9.0.0",
                "1.9.0.0.R1",
                "tinkerpop-3.7.1",
            ),
            (
                "a line before the catalog, whose published release is a later build",
                "1.1.1.0",
                "1.1.1.0.R7",
                "tinkerpop-3.6.2",
            ),
            (
                "a version that already names its release",
                "1.4.8.0.R2",
                "1.4.8.0.R2",
                "tinkerpop-3.7.1",
            ),
            (
                "a value that is no version",
                "not-a-version",
                "not-a-version",
                "tinkerpop-3.6.2",
            ),
        ];
        for (case, provisioned, expected_version, expected_gremlin) in rows {
            let reported = EngineStatusRecord::default().reported(Some(&configuration(
                provisioned,
                false,
                false,
                false,
            )));
            assert_eq!(
                reported.db_engine_version, *expected_version,
                "case {case:?}: expected dbEngineVersion {expected_version:?}, got {:?}",
                reported.db_engine_version
            );
            assert_eq!(
                reported.gremlin_version, *expected_gremlin,
                "case {case:?}: expected gremlin {expected_gremlin:?}, got {:?}",
                reported.gremlin_version
            );
        }
    }

    /// RULE: the features a cluster's configuration settles report that configuration, and
    /// the rest report the engine's own defaults.
    #[test]
    fn the_reported_features_follow_the_cluster_configuration() {
        // Rows: the three switches a create settles, and the statuses they must report.
        let rows: &[(&str, bool, bool, bool)] = &[
            ("every switch off", false, false, false),
            ("IAM authentication on", true, false, false),
            ("the audit log on", false, true, false),
            ("the slow-query log on", false, false, true),
            ("every switch on", true, true, true),
        ];
        for (case, iam, audit, slow_query) in rows {
            let reported = EngineStatusRecord::default()
                .reported(Some(&configuration("1.4.8.0", *iam, *audit, *slow_query)));
            for (feature, expected) in [
                (IAM_AUTHENTICATION_FEATURE, *iam),
                (AUDIT_LOG_FEATURE, *audit),
                (SLOW_QUERY_LOGS_FEATURE, *slow_query),
            ] {
                let answered = reported.features.get(feature);
                assert_eq!(
                    answered,
                    Some(&FeatureReport::status(expected)),
                    "case {case:?}: expected {feature} {}, got {answered:?}",
                    feature_status(expected)
                );
            }
            // The result cache reports a document of its own, whatever the cluster carries.
            let answered = reported.features.get(RESULT_CACHE_FEATURE);
            assert_eq!(
                answered,
                Some(&FeatureReport::status_document(false)),
                "case {case:?}: expected the result cache to report its own document, got \
                 {answered:?}"
            );
        }

        // With no cluster provisioned the record is reported exactly as it stands.
        let record = EngineStatusRecord::default();
        let reported = record.reported(None);
        assert_eq!(
            (reported.features, reported.db_engine_version),
            (record.features.clone(), record.db_engine_version.clone()),
            "case \"no cluster provisioned\": expected the engine's own defaults"
        );
    }

    /// RULE: a /status instant is rendered `EEE MMM dd HH:mm:ss UTC yyyy`.
    ///
    /// The rows span the weekday (seven consecutive days, so every name is exercised), the
    /// month name, a zero-padded day and a sub-second instant whose fraction the form drops.
    #[test]
    fn an_engine_status_instant_is_rendered_the_way_status_renders_one() {
        let rows: &[(&str, u64, &str)] = &[
            ("the epoch itself", 0, "Thu Jan 01 00:00:00 UTC 1970"),
            ("the day after it", 86_400, "Fri Jan 02 00:00:00 UTC 1970"),
            ("two days after it", 172_800, "Sat Jan 03 00:00:00 UTC 1970"),
            ("three days after it", 259_200, "Sun Jan 04 00:00:00 UTC 1970"),
            ("four days after it", 345_600, "Mon Jan 05 00:00:00 UTC 1970"),
            ("five days after it", 432_000, "Tue Jan 06 00:00:00 UTC 1970"),
            ("six days after it", 518_400, "Wed Jan 07 00:00:00 UTC 1970"),
            (
                "an instant in another month",
                1_762_709_114,
                "Sun Nov 09 17:25:14 UTC 2025",
            ),
            ("a leap day", 1_709_164_800, "Thu Feb 29 00:00:00 UTC 2024"),
        ];
        for (case, seconds, expected) in rows {
            let rendered = engine_timestamp(UNIX_EPOCH + Duration::from_secs(*seconds));
            assert_eq!(
                rendered, *expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }

        // A fraction of a second is not part of the form.
        let rendered = engine_timestamp(UNIX_EPOCH + Duration::from_micros(851_648));
        assert_eq!(
            rendered, "Thu Jan 01 00:00:00 UTC 1970",
            "case \"a sub-second instant\": expected the whole second, got {rendered:?}"
        );
    }

    /// RULE: a lab-mode parameter reports the value its own KIND states — a switch reports
    /// the on/off word, and a parameter whose name states a quantity reports that quantity,
    /// or no value at all while none is in force.
    #[test]
    fn a_lab_mode_parameter_reports_the_value_its_kind_states() {
        let rows: &[(&str, LabModeParameter, &str)] = &[
            (
                "a switch that is on",
                LabModeParameter::Switch(true),
                "enabled",
            ),
            (
                "a switch that is off",
                LabModeParameter::Switch(false),
                "disabled",
            ),
            ("no size in force", LabModeParameter::Size(None), ""),
            ("the smallest size", LabModeParameter::Size(Some(1)), "1"),
            ("a size of no units", LabModeParameter::Size(Some(0)), "0"),
            (
                "a size in force",
                LabModeParameter::Size(Some(1_024)),
                "1024",
            ),
            (
                "a size past every 32-bit one",
                LabModeParameter::Size(Some(u64::from(u32::MAX) + 1)),
                "4294967296",
            ),
        ];
        for (case, parameter, expected) in rows {
            let reported = parameter.reported();
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }

    /// A cluster configuration the control plane could have recorded.
    fn configuration(
        engine_version: &str,
        iam: bool,
        audit: bool,
        slow_query: bool,
    ) -> GraphClusterConfiguration {
        GraphClusterConfiguration {
            engine_version: engine_version.to_string(),
            iam_database_authentication_enabled: iam,
            audit_log_enabled: audit,
            slow_query_log_enabled: slow_query,
            provisioned_at: UNIX_EPOCH,
        }
    }
}
