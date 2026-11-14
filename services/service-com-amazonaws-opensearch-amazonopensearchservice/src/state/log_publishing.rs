// SPDX-License-Identifier: CC0-1.0

//! LOG PUBLISHING: what a domain's enabled `LogPublishingOptions` DELIVER into the
//! CloudWatch Logs log groups they name.
//!
//! `LogPublishingOptions` is "options to publish OpenSearch logs to Amazon CloudWatch
//! Logs", and a `LogPublishingOption` is "whether the log should be published" plus "the
//! ARN of the CloudWatch Logs group to publish logs to". Storing that map and reporting it
//! back through `DescribeDomain` is NOT that behaviour: the named group has to RECEIVE the
//! records, or a domain that publishes its logs and one that does not read the same to every
//! client — a dashboard over the group, a metric filter counting slow queries, an operator
//! looking for the query that was slow.
//!
//! # What produces a record
//!
//! The work on the domain's own data plane does, at the instant it is served
//! ([`crate::service::domain_rest`]), and each log type carries the records it is the log
//! OF:
//!
//! * [`LOG_TYPE_SEARCH_SLOW`] — one record per index a search or a count read, per phase of
//!   that search (its query phase and its fetch phase), when the read took at least as long
//!   as the index's own threshold for that phase ([`SEARCH_THRESHOLD_SETTING`],
//!   [`FETCH_THRESHOLD_SETTING`]);
//! * [`LOG_TYPE_INDEX_SLOW`] — one record per document write, when the write took at least
//!   as long as the index's own indexing slow-log threshold ([`INDEX_THRESHOLD_SETTING`]);
//! * [`LOG_TYPE_APPLICATION`] — the cluster's own line for a change to the indexes it holds
//!   (an index created, its settings updated, an index deleted), which is what the engine's
//!   application log records and needs no threshold.
//!
//! A threshold is an engine TIME VALUE (`"0ms"`, `"500ms"`, `"2s"`), and the engine's own
//! default `-1` disables the level: a level the index sets no threshold for logs nothing, so
//! an index whose thresholds were never set publishes no slow-log record however much a
//! domain publishes that log type. The DURATION compared against it is the one the emulator
//! really took to serve the operation — nothing is logged for work that was not slow, and no
//! duration is invented for work nothing ran.
//!
//! The levels are the engine's four (`warn`, `info`, `debug`, `trace`), and a record is
//! stated at the HIGHEST level whose threshold the operation reached, exactly as the engine
//! states it.
//!
//! # Where it goes
//!
//! Into the CloudWatch Logs service's OWN store in the SAME shared engine, through that
//! service's own record types, so what was published is exactly what `logs:FilterLogEvents`,
//! `logs:GetLogEvents` and `logs:DescribeLogStreams` report — the way Step Functions
//! delivers its history events to a log group and AWS Directory Service forwards its
//! security log. It is written there directly rather than sent over `logs:PutLogEvents`
//! because the request that produced the record is served under this service's own store
//! guard and answers in one step.
//!
//! A record comes FROM a node of the domain's cluster, so the stream it is written to is
//! named for that node — the `NodeId` of its first data node
//! ([`crate::state::logging_node_id`]), which is the id `DescribeDomainNodes` publishes for
//! it — and a reader can tell which node logged what.
//!
//! Delivery is BEST EFFORT, as publishing to a customer-owned destination is: a group
//! CloudWatch Logs does not hold receives nothing (nothing here creates one — the option
//! names a group the account already owns, which is why an enabled option is refused unless
//! that group exists, see [`super::log_destinations`]), and a group with no room for another
//! stream takes no further record. Either way the request that produced the record still
//! answers its caller, which asked for none of this; the loss is traced.
//!
//! LOCK ORDER: the records are BUILT while this service's store is held and DELIVERED
//! ([`deliver`]) once it is released, so the two stores are never held at once.

use ::std::time::{Duration, SystemTime};

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::{epoch_millis, index_settings, logging_node_id, Domain};

/// `LogType`'s `SEARCH_SLOW_LOGS` — the log of searches the cluster found slow.
pub const LOG_TYPE_SEARCH_SLOW: &str = "SEARCH_SLOW_LOGS";

/// `LogType`'s `INDEX_SLOW_LOGS` — the log of indexing operations it found slow.
pub const LOG_TYPE_INDEX_SLOW: &str = "INDEX_SLOW_LOGS";

/// `LogType`'s `ES_APPLICATION_LOGS` — the cluster's own application log.
pub const LOG_TYPE_APPLICATION: &str = "ES_APPLICATION_LOGS";

/// The index setting whose value is the search slow-log threshold of one LEVEL for the
/// QUERY phase, with the level's own name appended:
/// `index.search.slowlog.threshold.query.<level>`, as the engine names it. Read below
/// [`super::SETTINGS_INDEX_SECTION`], so the prefix the full name carries is not part of it.
pub const SEARCH_THRESHOLD_SETTING: &str = "search.slowlog.threshold.query";

/// The same for the FETCH phase of a search:
/// `index.search.slowlog.threshold.fetch.<level>`. A search is two phases, each with its
/// own threshold and its own logger, so a read slow enough for both is recorded by both.
pub const FETCH_THRESHOLD_SETTING: &str = "search.slowlog.threshold.fetch";

/// The same for an indexing operation: `index.indexing.slowlog.threshold.index.<level>`.
pub const INDEX_THRESHOLD_SETTING: &str = "indexing.slowlog.threshold.index";

/// The engine's four slow-log levels, HIGHEST first — the levels a threshold is set per and
/// a record is stated at. A record is stated at the first of them the operation reached.
const LEVELS: [&str; 4] = ["warn", "info", "debug", "trace"];

/// The value an engine time-value setting carries when the level it belongs to is OFF,
/// which is the engine's own default for every slow-log threshold: no operation is slow
/// enough for a negative threshold, so a level set to it logs nothing.
const DISABLED_THRESHOLD: &str = "-1";

/// The logger the engine states a slow search under, per phase.
const SEARCH_SLOWLOG_LOGGER: &str = "index.search.slowlog.query";

/// The logger it states a slow fetch phase under ([`SEARCH_SLOWLOG_LOGGER`]).
const FETCH_SLOWLOG_LOGGER: &str = "index.search.slowlog.fetch";

/// The logger it states a slow indexing operation under.
const INDEX_SLOWLOG_LOGGER: &str = "index.indexing.slowlog.index";

/// The level an application-log line for a change the cluster applied is stated at.
const APPLICATION_LEVEL: &str = "INFO";

/// How much of the operation's own source one slow-log record states: the engine's
/// documented `index.indexing.slowlog.source` default, which logs the first 1000
/// characters of it.
///
/// It is also what keeps the records one request produces bounded: a request may name many
/// indexes, each with its own threshold and so its own record, and the source is the body
/// the caller sent.
const MAX_SOURCE_CHARS: usize = 1_000;

/// The source one record states: the operation's own body on ONE line — the insignificant
/// whitespace between its JSON tokens dropped, which is the shared minification every
/// service that reports a JSON document back applies — cut to [`MAX_SOURCE_CHARS`] at a
/// character boundary.
fn stated_source(source: &str) -> String {
    let source = ::service_common::json_document::minified(source);
    match source.char_indices().nth(MAX_SOURCE_CHARS) {
        Some((at, _)) => source[..at].to_string(),
        None => source,
    }
}

/// One thing the data plane DID, which the domain's log publishing may have a record of.
///
/// Built by the plane that served the request ([`crate::service::domain_rest`]), out of what
/// it served and how long it took; which log type carries it, whether it is slow enough to
/// be recorded at all and how the record reads are this module's ([`records`]).
#[derive(Debug, Clone)]
pub enum Served<'a> {
    /// A search or a count read one index, taking `took`.
    Searched {
        /// The index that was read — the thresholds are the INDEX's own, so one request
        /// over several indexes is one of these per index.
        index: &'a str,
        /// How long serving it really took.
        took: Duration,
        /// The query the request carried, as it was written.
        source: &'a str,
    },
    /// One document was written into one index, taking `took`.
    Indexed {
        /// The index written into.
        index: &'a str,
        /// The `_id` the document was written under.
        id: &'a str,
        /// How long serving it really took.
        took: Duration,
        /// The document the request carried, as it was written.
        source: &'a str,
    },
    /// The cluster applied a change to the indexes it holds, which its application log
    /// states however long it took.
    Applied {
        /// The index the change was about.
        index: &'a str,
        /// What the cluster did to it.
        change: IndexChange,
    },
}

/// A change the cluster applied to the indexes it holds — what its own application log
/// states a line about, and the only work that is logged whatever it took.
///
/// The data plane says WHICH change it applied; the logger each one is stated under and the
/// words of its line are this module's, so the application log reads alike whichever route
/// made the change.
#[derive(Debug, Clone)]
pub enum IndexChange {
    /// An index the cluster did not hold was created, with the shard layout its settings
    /// give it.
    Created {
        /// `index.number_of_shards` of the created index.
        shards: i64,
        /// `index.number_of_replicas` of it.
        replicas: i64,
    },
    /// An index's settings were updated, naming the settings the update wrote.
    SettingsUpdated {
        /// The full names of the settings written, as an index setting is named.
        settings: Vec<String>,
    },
    /// An index was deleted.
    Deleted,
}

impl IndexChange {
    /// The cluster-state service that applies this change, which is the logger its line is
    /// stated under.
    fn logger(&self) -> &'static str {
        match self {
            Self::Created { .. } => "o.o.c.m.MetadataCreateIndexService",
            Self::SettingsUpdated { .. } => "o.o.c.m.MetadataUpdateSettingsService",
            Self::Deleted => "o.o.c.m.MetadataDeleteIndexService",
        }
    }

    /// What the line says about `index`, as the engine's own line words it.
    fn stated(&self, index: &str) -> String {
        match self {
            Self::Created { shards, replicas } => {
                format!("[{index}] creating index, cause [api], shards [{shards}]/[{replicas}]")
            }
            Self::SettingsUpdated { settings } => {
                format!("[{index}] updating index settings [{}]", settings.join(", "))
            }
            Self::Deleted => format!("[{index}] deleting index"),
        }
    }
}

/// One record a domain's log publishing delivers, with everything the write into the
/// destination group needs.
///
/// Built while this service's store is held and delivered once it is released, so nothing
/// here reaches back into the domain it came from.
#[derive(Debug, Clone)]
pub struct PublishedRecord {
    /// The log group the domain's option names — as the option's ARN spelled it, never a
    /// group of this module's own naming.
    pub log_group_name: String,
    /// The ARN of that group, as the option named it: which group of which account, Region
    /// and partition the record is for. The delivery resolves the group by its name and
    /// then checks its own ARN against this, exactly as the CREATE resolved the destination
    /// ([`super::log_destinations`]), so a record is never published to a local group that
    /// merely shares the name an option asked for.
    pub log_group_arn: String,
    /// The stream the record is written to: the `NodeId` of the node that logged it.
    pub log_stream_name: String,
    /// The Region the stream's ARN is stamped with: the domain's own.
    pub region: String,
    /// The record itself, as [`line`] writes it.
    pub message: String,
    /// When the operation happened — the event's timestamp, not its ingestion time.
    pub at: SystemTime,
}

/// The records `domain`'s enabled log-publishing options have of `served`, at `at`.
///
/// Empty for a domain that publishes none of the log types the work belongs to, and for an
/// operation no threshold of its index makes slow — which is the whole of "the group stays
/// empty".
pub fn records(domain: &Domain, served: &[Served<'_>], at: SystemTime) -> Vec<PublishedRecord> {
    // The node the records come from: the cluster that served the work has one whatever its
    // configuration states ([`logging_node_id`]).
    let node = logging_node_id(domain);
    let mut published = Vec::new();
    for one in served {
        match one {
            Served::Searched {
                index,
                took,
                source,
            } => {
                // A search is its query phase and its fetch phase, each with a threshold
                // and a logger of its own, so a read is recorded by every phase whose
                // threshold it reached.
                for (setting, logger) in [
                    (SEARCH_THRESHOLD_SETTING, SEARCH_SLOWLOG_LOGGER),
                    (FETCH_THRESHOLD_SETTING, FETCH_SLOWLOG_LOGGER),
                ] {
                    published.extend(slow_record(
                        domain,
                        index,
                        setting,
                        *took,
                        LOG_TYPE_SEARCH_SLOW,
                        logger,
                        &format!(
                            "[{index}] {}, total_shards[{}], source[{}]",
                            took_clause(*took),
                            shard_count(domain, index),
                            stated_source(source)
                        ),
                        &node,
                        at,
                    ));
                }
            }
            Served::Indexed {
                index,
                id,
                took,
                source,
            } => published.extend(slow_record(
                domain,
                index,
                INDEX_THRESHOLD_SETTING,
                *took,
                LOG_TYPE_INDEX_SLOW,
                INDEX_SLOWLOG_LOGGER,
                &format!(
                    "[{index}] {}, id[{id}], source[{}]",
                    took_clause(*took),
                    stated_source(source)
                ),
                &node,
                at,
            )),
            Served::Applied { index, change } => {
                published.extend(destination(domain, LOG_TYPE_APPLICATION).map(
                    |(log_group_arn, log_group_name)| PublishedRecord {
                        log_group_name,
                        log_group_arn,
                        log_stream_name: node.clone(),
                        region: domain.region.clone(),
                        message: line(
                            APPLICATION_LEVEL,
                            change.logger(),
                            &node,
                            &change.stated(index),
                            at,
                        ),
                        at,
                    },
                ));
            }
        }
    }
    published
}

/// The slow-log record one operation on `index` produces, or `None` when it produces none:
/// the domain does not publish `log_type`, or no threshold the index sets makes an operation
/// of `took` slow.
#[allow(clippy::too_many_arguments)]
fn slow_record(
    domain: &Domain,
    index: &str,
    threshold_setting: &str,
    took: Duration,
    log_type: &str,
    logger: &str,
    body: &str,
    node: &str,
    at: SystemTime,
) -> Option<PublishedRecord> {
    let (log_group_arn, log_group_name) = destination(domain, log_type)?;
    let level = slow_level(domain, index, threshold_setting, took)?;
    Some(PublishedRecord {
        log_group_name,
        log_group_arn,
        log_stream_name: node.to_string(),
        region: domain.region.clone(),
        message: line(&level.to_uppercase(), logger, node, body, at),
        at,
    })
}

/// The log group `domain` publishes `log_type` to — its ARN and its name — or `None` when
/// it does not publish that log type at all: no option for it, an option that is not
/// enabled, or one naming no log group.
///
/// WHICH group an option's ARN names is the one reading [`super::log_destinations`] resolved
/// the destination with when the option was accepted, so the group a record goes to is the
/// group the create checked exists.
fn destination(domain: &Domain, log_type: &str) -> Option<(String, String)> {
    let option = domain.log_publishing_options.get(log_type)?;
    if !option.enabled.unwrap_or(false) {
        return None;
    }
    let arn = option.cloud_watch_logs_log_group_arn.as_deref()?;
    let (arn, name) = super::log_destinations::named_log_group(arn)?;
    Some((arn.to_string(), name.to_string()))
}

/// The level a `took` of this length is slow at on `index`, from the thresholds the index's
/// own settings state for `setting` — the HIGHEST level whose threshold it reached, and
/// `None` when it reached none of them.
fn slow_level(
    domain: &Domain,
    index: &str,
    setting: &str,
    took: Duration,
) -> Option<&'static str> {
    let settings = index_settings(&domain.indexes.get(index)?.schema);
    LEVELS.into_iter().find(|level| {
        setting_text(&settings, &format!("{setting}.{level}"))
            .as_deref()
            .and_then(engine_duration)
            .is_some_and(|threshold| took >= threshold)
    })
}

/// The text the settings `settings` state for the setting `name`, under the dotted name it
/// has or under the sections that name expands to — the two spellings a caller may have
/// written it in. A number or a boolean reads as the text it was written as, since every
/// engine setting is a string.
fn setting_text(
    settings: &::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Option<String> {
    use ::serde_json::Value;

    let mut at = settings.get(name);
    if at.is_none() {
        let mut node: Option<&Value> = None;
        for segment in name.split('.') {
            node = match node {
                None => settings.get(segment),
                Some(value) => value.get(segment),
            };
            node?;
        }
        at = node;
    }
    match at? {
        Value::String(text) => Some(text.clone()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
        value => Some(value.to_string()),
    }
}

/// The duration an engine TIME VALUE states, or `None` for a value that states no duration
/// at all — a negative one, which is how a slow-log level is turned off
/// ([`DISABLED_THRESHOLD`]), and a value in no unit the engine takes.
///
/// The units are the engine's own time-value suffixes: `nanos`, `micros`, `ms`, `s`, `m`,
/// `h` and `d`. A bare number is milliseconds, which is what the engine reads one as.
fn engine_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.starts_with('-') || value == DISABLED_THRESHOLD {
        return None;
    }
    for (suffix, unit) in [
        ("nanos", Duration::from_nanos(1)),
        ("micros", Duration::from_micros(1)),
        ("ms", Duration::from_millis(1)),
        ("s", Duration::from_secs(1)),
        ("m", Duration::from_secs(60)),
        ("h", Duration::from_secs(60 * 60)),
        ("d", Duration::from_secs(24 * 60 * 60)),
    ] {
        if let Some(count) = value.strip_suffix(suffix) {
            return count.trim().parse::<u64>().ok().map(|count| {
                // A value a caller wrote can name a duration no `Duration` holds, and
                // multiplying past that ceiling PANICS — so the product is taken checked
                // and a value beyond it reads as the longest duration there is, which no
                // operation reaches either.
                unit.checked_mul(u32::try_from(count).unwrap_or(u32::MAX))
                    .unwrap_or(Duration::MAX)
            });
        }
    }
    value.parse::<u64>().ok().map(Duration::from_millis)
}

/// How long an operation took, as the engine's slow log states it: the duration itself and
/// the whole milliseconds of it, which is the figure a reader filters on.
fn took_clause(took: Duration) -> String {
    format!(
        "took[{:.1}ms], took_millis[{}]",
        took.as_secs_f64() * 1_000.0,
        took.as_millis()
    )
}

/// How many shards the index `index` of `domain` has, as the settings it carries state —
/// one for an index whose settings state none, which is the cluster's own default.
fn shard_count(domain: &Domain, index: &str) -> i64 {
    domain
        .indexes
        .get(index)
        .and_then(|index| {
            setting_text(&index_settings(&index.schema), "number_of_shards")?
                .parse::<i64>()
                .ok()
        })
        .filter(|shards| *shards > 0)
        .unwrap_or(1)
}

/// One record's message: the instant, the level, the logger that stated it, the node it came
/// from and what it says — the fields the engine's own log lines carry, and nothing this
/// emulator holds no value behind.
fn line(level: &str, logger: &str, node: &str, body: &str, at: SystemTime) -> String {
    format!("[{}][{level}][{logger}] [{node}] {body}", stated_time(at))
}

/// The instant a record states, in the ISO-8601 form every timestamp this emulator writes
/// carries.
fn stated_time(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Write `records` into the CloudWatch Logs store of the SAME engine.
///
/// Best effort: a destination group that is not there, or that has no room for another
/// stream, loses the record and is traced (see this module's docs). Call it with NO store of
/// this service locked.
pub fn deliver<Eng: Send + Sync + 'static>(engine: &Eng, records: &[PublishedRecord]) {
    if records.is_empty() {
        return;
    }
    let Some(state) = logs::service_state(engine) else {
        ::tracing::warn!(
            "no CloudWatch Logs store is mounted, so a domain's published log records were \
             not delivered"
        );
        return;
    };
    let mut store = state.lock();
    for record in records {
        let ingested_at = logs::now_ms();
        // The data identifiers the data protection policies in force for the group name: such
        // a policy masks what is ingested into the group, by whichever path the records
        // arrive.
        let data_identifiers = store
            .log_groups
            .get(&record.log_group_name)
            .map(|group| store.data_protection_identifiers_for(group))
            .unwrap_or_default();
        let stored = logs::StoredEvent {
            timestamp: epoch_millis(record.at),
            masked_message: logs::data_protection::masked(&record.message, &data_identifiers),
            message: record.message.clone(),
            ingestion_time: ingested_at,
            ingestion_seq: store.take_ingestion_seq(),
        };
        let added = stored.stored_bytes();
        let Some(group) = store
            .log_groups
            .get_mut(&record.log_group_name)
            .filter(|group| group.log_group_arn == record.log_group_arn)
        else {
            // The option names a group the ACCOUNT owns; nothing here creates one, so a
            // group that is no longer there — or a local group of that name that is not the
            // one the ARN named — is a destination nothing reaches.
            ::tracing::warn!(
                log_group = %record.log_group_name,
                "the log group a domain publishes to is not held by CloudWatch Logs, so a \
                 published log record was not delivered"
            );
            continue;
        };
        // A stream is opened per NODE, and it is admitted against the ceiling the owning
        // service's own `CreateLogStream` admits one against, so no write path can take a
        // group past it.
        if !group.streams.contains_key(&record.log_stream_name)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
        {
            ::tracing::warn!(
                log_group = %record.log_group_name,
                log_stream = %record.log_stream_name,
                "the log group already holds the maximum number of streams, so a published \
                 log record was not delivered"
            );
            continue;
        }
        let stream = group
            .streams
            .entry(record.log_stream_name.clone())
            .or_insert_with(|| {
                logs::LogStreamRecord::new(
                    &record.region,
                    &record.log_group_name,
                    &record.log_stream_name,
                    ingested_at,
                )
            });
        stream.events.push(stored);
        // The stream is kept in the `(timestamp, ingestion_seq)` order its reader pages it
        // in; records arrive in that order already, so only one predating what is there
        // moves.
        stream.events.sort_by_key(logs::StoredEvent::key);
        let discarded = stream.enforce_storage_ceiling();
        stream.refresh_event_times();
        stream.last_ingestion_time = Some(ingested_at);
        group.stored_bytes = group
            .stored_bytes
            .saturating_add(added)
            .saturating_sub(discarded)
            .max(0);
    }
}
