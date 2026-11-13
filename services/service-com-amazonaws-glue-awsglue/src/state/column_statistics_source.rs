// SPDX-License-Identifier: CC0-1.0

//! Where a column statistics task run READS the table's data from, and how it resolves
//! the objects behind it into the cells of the columns it covers.
//!
//! A column statistics task run is a computation over the data a Data Catalog table
//! describes: the objects under its `StorageDescriptor$Location`, laid out the way its
//! `StorageDescriptor$SerdeInfo` says. So a run reads those objects out of the emulator's
//! OWN Amazon S3, through the framework's in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — the same transport a client's
//! own S3 request takes, so a run computes over exactly the bytes that were uploaded.
//! Nothing here opens a socket, resolves a host or reads an ambient credential.
//!
//! The run is PERFORMED where it is started, with no store guard held, and what it
//! computed travels with the run record ([`ColumnStatisticsRunOutcome`]): the clock then
//! owes the run its terminal status one span later, and settling it is what publishes the
//! statistics onto the table — so a caller that reads the run while it is still in flight
//! sees no statistics, and a run that reached its terminal status reports exactly the
//! figures its own reading produced.
//!
//! HOW MUCH IS READ IS BOUNDED, before any of it is taken into this process: how many
//! objects a run reads ([`MAX_COLUMN_STATISTICS_OBJECTS`]), how large one of them may be
//! and how large they may be together, and how many rows one run aggregates
//! ([`MAX_COLUMN_STATISTICS_ROWS`]). A table whose data is past any of those bounds is one
//! this run could not read, which is a run that generated no statistics rather than one
//! that reports partial ones.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

use super::*;

/// What a column statistics task run's computation over the table's data produced.
///
/// Recorded on the run ([`ColumnStatisticsTaskRunEntry::outcome`]) and applied when the
/// clock settles it, so the status a run reaches is decided by what the reading answered
/// and not by the operation that read it.
#[derive(Debug, Clone)]
pub enum ColumnStatisticsRunOutcome {
    /// The data was read and these are the statistics of the covered columns, which the
    /// run publishes onto the table when it settles. A column the run covered whose
    /// declared type has no column-level statistics is not among them.
    Computed(Vec<ComputedColumnStatistics>),
    /// The statistics could not be computed, for the reason the run reports in
    /// `ColumnStatisticsTaskRun$ErrorMessage`.
    Failed(String),
}

/// `ColumnStatisticsTaskRun$ErrorMessage` of a run over a table that states no storage
/// location.
///
/// Statistics are generated from the table's DATA, and a table whose
/// `StorageDescriptor$Location` is absent or empty points at none, so there is nothing for
/// a run to read. Whole fixed text: it names no value of the request, because the table it
/// is about is the one the caller addressed.
pub const COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION: &str =
    "The table has no storage location, so no column statistics could be generated for it.";

/// `ColumnStatisticsTaskRun$ErrorMessage` of a run whose table's data could not be read:
/// the location holds no object this run could read, or the object store answered that it
/// does not serve it.
///
/// The variable part is the location the statistics would have been computed from, read
/// off the stored table. WHY the data could not be read is logged against the run rather
/// than reported to the caller.
pub fn column_statistics_task_run_unreadable_data(location: &str) -> String {
    format!(
        "The table's data at {location} could not be read, so no column statistics could be \
         generated for it."
    )
}

/// `ColumnStatisticsTaskRun$ErrorMessage` of a run that was not carried out because too
/// many were already generating statistics: the condition is on the SERVICE, not on the
/// table, so the reason says so rather than claiming the table's data could not be read.
pub const COLUMN_STATISTICS_TASK_RUN_TOO_MANY_RUNNING: &str =
    "Too many column statistics task runs are in progress, so this run did not generate \
     statistics.";

/// `ColumnStatisticsTaskRun$ErrorMessage` of a run that ran out of time before it had read
/// the table's data. Also a condition on the run rather than on the table, for the reason
/// [`COLUMN_STATISTICS_TASK_RUN_TOO_MANY_RUNNING`] gives.
pub const COLUMN_STATISTICS_TASK_RUN_TIMED_OUT: &str =
    "The column statistics task run timed out before it finished generating statistics.";

/// How many objects one column statistics task run reads.
///
/// CHOSEN, not an AWS quota: a run walks the objects under a table's location, and that
/// prefix is written by callers, so the walk is bounded here. A location holding more is
/// data this run did not read.
pub const MAX_COLUMN_STATISTICS_OBJECTS: usize = 500;

/// How large ONE object a run reads may be, checked against the size the listing DECLARES
/// before the object is fetched. CHOSEN, for the reason
/// [`MAX_COLUMN_STATISTICS_OBJECTS`] gives.
pub const MAX_COLUMN_STATISTICS_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// How large the objects of ONE run may be together. CHOSEN, for the reason
/// [`MAX_COLUMN_STATISTICS_OBJECTS`] gives.
pub const MAX_COLUMN_STATISTICS_TOTAL_BYTES: usize = 32 * 1024 * 1024;

/// How many rows one run aggregates. CHOSEN, for the reason
/// [`MAX_COLUMN_STATISTICS_OBJECTS`] gives: it also bounds the distinct-value set of every
/// column the run covers.
pub const MAX_COLUMN_STATISTICS_ROWS: usize = 200_000;

/// How many CELLS one run aggregates: the product of the rows it reads and the columns it
/// covers, which is the work the aggregation really does — neither factor bounds it alone,
/// because a caller writes both the objects under a location and the column list of the
/// table above it.
///
/// CHOSEN, for the reason [`MAX_COLUMN_STATISTICS_OBJECTS`] gives. A run whose data is past
/// it is a run that could not read that data, rather than one reporting the statistics of
/// part of it.
pub const MAX_COLUMN_STATISTICS_CELLS: usize = 2_000_000;

/// How often the aggregation hands the runtime back: once per this many rows.
///
/// The aggregation is this emulator's own synchronous work on the request's task, so it
/// yields often enough that the run's budget, a cancellation and every other request can
/// land while it goes on, and seldom enough that the yield is not the cost of a row.
pub const YIELD_EVERY_ROWS: usize = 1_000;

/// How long ONE run may spend reading and aggregating the table's data.
///
/// CHOSEN, not an AWS duration: it is the wall clock this emulator gives its own reading,
/// so a location whose objects cannot be walked inside it ends the run rather than holding
/// a request open.
pub const COLUMN_STATISTICS_RUN_BUDGET: Duration = Duration::from_secs(10);

/// How many column statistics task runs read data at once.
///
/// CHOSEN: a run is in flight per table, so without this the bounds above would be
/// multiplied by the number of tables an account holds. Two runs read at a time and the
/// rest wait for a permit.
pub const MAX_CONCURRENT_COLUMN_STATISTICS_RUNS: usize = 2;

/// How long a run waits for one of [`MAX_CONCURRENT_COLUMN_STATISTICS_RUNS`].
///
/// CHOSEN: the wait is bounded, so a start answers either way rather than queueing without
/// end; a run that waited this long reports that nothing read its data.
pub const COLUMN_STATISTICS_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The permits [`MAX_CONCURRENT_COLUMN_STATISTICS_RUNS`] hands out, for the whole process:
/// the bound is on how much data this emulator reads at once, whichever table or caller
/// asked — so every read this service makes of a table's data takes one, the rows a data
/// quality evaluation run reads included ([`super::table_rows`]).
pub(super) static COLUMN_STATISTICS_RUN_PERMITS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_COLUMN_STATISTICS_RUNS);

/// The Hive field delimiter of a delimited-text table that states none.
///
/// The delimiter is a serde property (`field.delim` of the lazy-simple serde,
/// `separatorChar` of the open-CSV one); a table that states neither is read with Hive's
/// own default field separator, `\001`, which is what a text table written without a
/// stated delimiter carries.
pub const DEFAULT_HIVE_FIELD_DELIMITER: u8 = 0x01;

/// The Hive null marker of a table that states none: the value `serialization.null.format`
/// defaults to, which is the text a text-format table writes a NULL cell as.
pub const DEFAULT_HIVE_NULL_MARKER: &str = "\\N";

/// The serde property carrying the field delimiter of a lazy-simple (Hive text) table.
const FIELD_DELIMITER_PROPERTY: &str = "field.delim";
/// The serde property carrying the field delimiter of an open-CSV table.
const SEPARATOR_CHARACTER_PROPERTY: &str = "separatorChar";
/// The serde property carrying the quote character of an open-CSV table.
const QUOTE_CHARACTER_PROPERTY: &str = "quoteChar";
/// The serde property carrying the text a NULL cell is written as.
const NULL_FORMAT_PROPERTY: &str = "serialization.null.format";
/// The table property carrying how many leading lines of each object are a header rather
/// than data.
const SKIP_HEADER_LINE_COUNT_PROPERTY: &str = "skip.header.line.count";

/// The marker of an input format whose objects are LINES OF TEXT, which is the layout a
/// run reads: the Hadoop text input format, which every Hive-style delimited or
/// JSON-per-line table is stored under.
const TEXT_INPUT_FORMAT_MARKER: &str = "textinputformat";

/// The marker of a serialization library that writes each row as a JSON document.
const JSON_SERDE_MARKER: &str = "json";

/// The marker of the serialization library that reads quoted comma-separated text, which
/// is the one layout where a quote character is not data.
const OPEN_CSV_SERDE_MARKER: &str = "opencsvserde";

/// One column a run covers: its declared name and type, plus where its cell sits in a row
/// of delimited text.
#[derive(Debug, Clone)]
pub struct PlannedColumn {
    /// The column's name as the stored table declares it.
    pub name: String,
    /// `Column$Type` as the stored table declares it.
    pub declared_type: String,
    /// The column's position in `StorageDescriptor$Columns`, which is the field of a
    /// delimited row that holds its cell.
    pub position: usize,
}

/// How the objects behind a table are laid out, read off its storage descriptor.
#[derive(Debug, Clone)]
pub struct TableDataLayout {
    /// Whether each line is a JSON document whose members are the columns, rather than a
    /// row of delimited fields.
    pub json: bool,
    /// The byte that separates one field of a delimited row from the next.
    pub delimiter: u8,
    /// The quote character of a quoted layout, or `None` where a quote is data.
    pub quote: Option<u8>,
    /// The text a NULL cell is written as.
    pub null_marker: String,
    /// How many leading lines of each object are a header rather than data.
    pub header_lines: usize,
    /// Whether the table's `InputFormat` says its objects are lines of text.
    pub text_input_format: bool,
}

impl TableDataLayout {
    /// The layout the stored `table` describes.
    pub(super) fn of(table: &TableEntry) -> Self {
        let descriptor = table.storage_descriptor.as_ref();
        let serde = descriptor.and_then(|descriptor| descriptor.serde_info.as_ref());
        let library = serde
            .and_then(|serde| serde.serialization_library.as_deref())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let properties = serde.and_then(|serde| serde.parameters.as_ref());
        let stated_delimiter = property(properties, FIELD_DELIMITER_PROPERTY)
            .or_else(|| property(properties, SEPARATOR_CHARACTER_PROPERTY))
            .and_then(single_byte);
        Self {
            json: library.contains(JSON_SERDE_MARKER),
            delimiter: stated_delimiter.unwrap_or(DEFAULT_HIVE_FIELD_DELIMITER),
            // Only the quoted layout HAS a quote character: in a Hive text table a quote
            // is an ordinary character of the data.
            quote: library
                .contains(OPEN_CSV_SERDE_MARKER)
                .then(|| {
                    property(properties, QUOTE_CHARACTER_PROPERTY)
                        .and_then(single_byte)
                        .unwrap_or(b'"')
                }),
            null_marker: property(properties, NULL_FORMAT_PROPERTY)
                .unwrap_or_else(|| DEFAULT_HIVE_NULL_MARKER.to_string()),
            // The header count is a property of the TABLE, and a storage descriptor
            // carries its own property map too, so BOTH are read — the descriptor's
            // first, because it describes the objects, and the table's where a create put
            // it beside the table itself.
            header_lines: property(
                descriptor.and_then(|descriptor| descriptor.parameters.as_ref()),
                SKIP_HEADER_LINE_COUNT_PROPERTY,
            )
            .or_else(|| property(table.parameters.as_ref(), SKIP_HEADER_LINE_COUNT_PROPERTY))
            .and_then(|count| count.trim().parse::<usize>().ok())
            .unwrap_or(0),
            text_input_format: descriptor
                .and_then(|descriptor| descriptor.input_format.as_deref())
                .map(|format| {
                    format
                        .to_ascii_lowercase()
                        .replace('.', "")
                        .contains(TEXT_INPUT_FORMAT_MARKER)
                })
                // A table that states no input format states nothing against reading its
                // objects as lines of text, which is the layout a Hive-style table is
                // stored in.
                .unwrap_or(true),
        }
    }
}

/// What a column statistics task run has to do: which columns it covers, where their data
/// is and how it is laid out.
///
/// Built from the STORED table under the store's guard and then carried out with no guard
/// held, so the run computes over the table as it stood when it started.
#[derive(Debug, Clone)]
pub struct ColumnStatisticsRunPlan {
    /// The columns the run covers that HAVE column-level statistics, in the order the
    /// table declares them.
    pub columns: Vec<PlannedColumn>,
    /// `StorageDescriptor$Location`: where the table's data lives.
    pub location: Option<String>,
    /// How the objects there are laid out.
    pub layout: TableDataLayout,
}

impl ColumnStatisticsRunPlan {
    /// The plan of a run of `table` covering `requested`.
    ///
    /// A requested name the table does not declare is not part of the plan: the statistics
    /// of a column are of a column the table HAS. Nor is a column whose declared type has
    /// no column-level statistics ([`column_statistics_type_of`]) — a complex type — so a
    /// run over one generates none for it. Names are matched the way the catalog matches a
    /// column name, without regard to case.
    pub fn of(table: &TableEntry, requested: &[String]) -> Self {
        let declared: Vec<&Column> = table
            .storage_descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.columns.as_ref())
            .map(|columns| columns.iter().collect())
            .unwrap_or_default();
        let wanted: HashSet<String> = requested.iter().map(|name| fold_name(name)).collect();
        let columns = declared
            .iter()
            .enumerate()
            .filter(|(_, column)| wanted.contains(&fold_name(&column.name)))
            .filter_map(|(position, column)| {
                let declared_type = column.r#type.clone().unwrap_or_default();
                column_statistics_type_of(&declared_type).map(|_| PlannedColumn {
                    name: column.name.clone(),
                    declared_type,
                    position,
                })
            })
            .collect();
        Self {
            columns,
            location: table
                .storage_descriptor
                .as_ref()
                .and_then(|descriptor| descriptor.location.clone()),
            layout: TableDataLayout::of(table),
        }
    }
}

/// Compute the statistics `plan` describes, reading the table's data out of the emulator's
/// own Amazon S3.
///
/// Awaited with NO store guard held. Every answer is grounded in what the object store
/// said: statistics only for data that was read, and the run's own failure reason for a
/// location that holds nothing readable.
///
/// BOUNDED in every direction a caller can grow: how many runs read at once
/// ([`MAX_CONCURRENT_COLUMN_STATISTICS_RUNS`], with a bounded wait for a permit), how long
/// one run may take ([`COLUMN_STATISTICS_RUN_BUDGET`]), how much it may read (the object
/// and byte bounds above) and how many CELLS it may aggregate
/// ([`MAX_COLUMN_STATISTICS_CELLS`], the product of rows and covered columns rather than
/// either factor alone). The aggregation yields to the runtime every
/// [`YIELD_EVERY_ROWS`] rows, so the budget and a cancellation can both land while it
/// runs.
pub async fn compute_column_statistics(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &ColumnStatisticsRunPlan,
) -> ColumnStatisticsRunOutcome {
    let Some(location) = plan
        .location
        .as_deref()
        .map(str::trim)
        .filter(|location| !location.is_empty())
    else {
        return ColumnStatisticsRunOutcome::Failed(
            COLUMN_STATISTICS_TASK_RUN_NO_TABLE_LOCATION.to_string(),
        );
    };
    let unreadable = |detail: &str| {
        // The table's own values are rendered in their escaped `Debug` form, so a
        // location carrying a newline cannot forge a log line.
        ::tracing::debug!(
            location = ?location,
            detail = %detail,
            "a Glue column statistics task run could not read the table's data"
        );
        ColumnStatisticsRunOutcome::Failed(column_statistics_task_run_unreadable_data(location))
    };

    if !plan.layout.text_input_format {
        return unreadable("the table's input format is not one whose objects are lines of text");
    }
    let Some((bucket, prefix)) = split_s3_location(location) else {
        return unreadable("the table's location does not name an Amazon S3 bucket");
    };

    // How many runs read data at once is bounded, so concurrent starts cannot multiply
    // the bounds below by the number of tables an account holds. The wait for a permit is
    // bounded too, and each holder is bounded by the budget. A run that waited the whole
    // wait says THAT, rather than claiming the table's data could not be read: the table
    // is fine, the service was busy.
    let permit = match ::tokio::time::timeout(
        COLUMN_STATISTICS_PERMIT_WAIT,
        COLUMN_STATISTICS_RUN_PERMITS.acquire(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => {
            ::tracing::debug!(
                location = ?location,
                "a Glue column statistics task run found no free reader for the table's data"
            );
            return ColumnStatisticsRunOutcome::Failed(
                COLUMN_STATISTICS_TASK_RUN_TOO_MANY_RUNNING.to_string(),
            );
        }
    };
    let client = s3_client(ctx, region);
    let aggregated = ::tokio::time::timeout(
        COLUMN_STATISTICS_RUN_BUDGET,
        aggregate(&client, &bucket, &prefix, plan),
    )
    .await;
    drop(permit);

    match aggregated {
        Ok(Ok(statistics)) => ColumnStatisticsRunOutcome::Computed(statistics),
        Ok(Err(detail)) => unreadable(&detail),
        // The run ran out of time rather than out of readable data, so it says so.
        Err(_elapsed) => {
            ::tracing::debug!(
                location = ?location,
                "a Glue column statistics task run did not read the table's data inside its budget"
            );
            ColumnStatisticsRunOutcome::Failed(COLUMN_STATISTICS_TASK_RUN_TIMED_OUT.to_string())
        }
    }
}

/// Read the objects under `prefix` and aggregate the columns `plan` covers, or answer why
/// the data could not be read.
async fn aggregate(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
    plan: &ColumnStatisticsRunPlan,
) -> Result<Vec<ComputedColumnStatistics>, String> {
    let objects = list_objects(client, bucket, prefix).await?;
    if objects.is_empty() {
        return Err("the table's location holds no object".to_string());
    }

    let mut accumulators: Vec<(PlannedColumn, ColumnStatisticsAccumulator)> = plan
        .columns
        .iter()
        .filter_map(|column| {
            ColumnStatisticsAccumulator::for_column(&column.name, &column.declared_type)
                .map(|accumulator| (column.clone(), accumulator))
        })
        .collect();
    // Which accumulator a named cell belongs to, folded once per run rather than once per
    // cell: a JSON row addresses its cells by NAME, and the catalog matches a column name
    // without regard to case.
    let by_folded_name: HashMap<String, usize> = accumulators
        .iter()
        .enumerate()
        .map(|(index, (column, _))| (fold_name(&column.name), index))
        .collect();
    let mut walked = WalkedData::default();
    for key in objects {
        let bytes = read_object(client, bucket, &key).await?;
        observe_object(
            &bytes,
            &plan.layout,
            &mut accumulators,
            &by_folded_name,
            &mut walked,
        )
        .await?;
    }

    Ok(accumulators
        .into_iter()
        .map(|(_, accumulator)| accumulator.finish())
        .collect())
}

/// How much of a run's budget the data walked so far has used: the rows read and the cells
/// observed, which are what [`MAX_COLUMN_STATISTICS_ROWS`] and
/// [`MAX_COLUMN_STATISTICS_CELLS`] bound across every object of the run.
#[derive(Debug, Default)]
struct WalkedData {
    rows: usize,
    cells: usize,
}

impl WalkedData {
    /// Count one row of `columns` covered columns, and answer why the run may not go on
    /// when either bound is reached.
    fn walk_row(&mut self, columns: usize) -> Result<(), String> {
        self.rows += 1;
        if self.rows > MAX_COLUMN_STATISTICS_ROWS {
            return Err(format!(
                "the location holds more than the {MAX_COLUMN_STATISTICS_ROWS} rows one run \
                 aggregates"
            ));
        }
        self.cells = self.cells.saturating_add(columns);
        if self.cells > MAX_COLUMN_STATISTICS_CELLS {
            return Err(format!(
                "the covered columns of those rows are more than the \
                 {MAX_COLUMN_STATISTICS_CELLS} cells one run aggregates"
            ));
        }
        Ok(())
    }

    /// Whether the runtime is owed a yield at this point in the walk.
    fn owes_yield(&self) -> bool {
        self.rows.is_multiple_of(YIELD_EVERY_ROWS)
    }
}

/// The keys of the objects under `prefix`, bounded by [`MAX_COLUMN_STATISTICS_OBJECTS`]
/// and the byte bounds beside it, which are decided from the sizes the LISTING declares —
/// before any object is fetched.
///
/// `Err` is why the data could not be read, for the run's log.
pub(super) async fn list_objects(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let mut listed: Vec<(String, usize)> = Vec::new();
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "the object store answered {:?} to the listing: {}",
                    error.code(),
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else { continue };
            // A key ending in the layout's own separator is a folder marker, not data.
            if key.ends_with('/') {
                continue;
            }
            let size = usize::try_from(object.size().unwrap_or_default().max(0)).unwrap_or(0);
            if size > MAX_COLUMN_STATISTICS_OBJECT_BYTES {
                return Err(format!(
                    "the object {key:?} is {size} bytes, past the \
                     {MAX_COLUMN_STATISTICS_OBJECT_BYTES} bytes one object of a run may hold"
                ));
            }
            listed.push((key.to_string(), size));
            if listed.len() > MAX_COLUMN_STATISTICS_OBJECTS {
                return Err(format!(
                    "the location holds more than the {MAX_COLUMN_STATISTICS_OBJECTS} objects \
                     one run reads"
                ));
            }
        }
        match page.next_continuation_token() {
            Some(token) => continuation = Some(token.to_string()),
            None => break,
        }
    }
    let total: usize = listed.iter().map(|(_, size)| *size).sum();
    if total > MAX_COLUMN_STATISTICS_TOTAL_BYTES {
        return Err(format!(
            "the location holds {total} bytes, past the {MAX_COLUMN_STATISTICS_TOTAL_BYTES} bytes \
             one run reads"
        ));
    }
    Ok(listed.into_iter().map(|(key, _)| key).collect())
}

/// The bytes of one object, bounded again by [`MAX_COLUMN_STATISTICS_OBJECT_BYTES`]: the
/// read asks for at most that many plus the one byte that shows the object is past it, so
/// the bound is enforced by the REQUEST and nothing larger is buffered.
pub(super) async fn read_object(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let object = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{MAX_COLUMN_STATISTICS_OBJECT_BYTES}"))
        .send()
        .await
        .map_err(|error| {
            format!(
                "the object store answered {:?} for the object {key:?}: {}",
                error.code(),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    let body = object
        .body
        .collect()
        .await
        .map_err(|error| format!("the object {key:?} did not arrive in full: {error}"))?
        .into_bytes();
    if body.len() > MAX_COLUMN_STATISTICS_OBJECT_BYTES {
        return Err(format!(
            "the object {key:?} is past the {MAX_COLUMN_STATISTICS_OBJECT_BYTES} bytes one object \
             of a run may hold"
        ));
    }
    Ok(body.to_vec())
}

/// Feed every row of one object's bytes to the accumulators, counting what the run has
/// walked so far against its bounds ([`WalkedData`]).
async fn observe_object(
    bytes: &[u8],
    layout: &TableDataLayout,
    accumulators: &mut [(PlannedColumn, ColumnStatisticsAccumulator)],
    by_folded_name: &HashMap<String, usize>,
    walked: &mut WalkedData,
) -> Result<(), String> {
    if layout.json {
        return observe_json_lines(bytes, layout, accumulators, by_folded_name, walked).await;
    }
    observe_delimited(bytes, layout, accumulators, walked).await
}

/// Read the object as rows of delimited text, one field per declared column.
///
/// The reader and the header-line handling are the ONE delimited reading this service makes
/// of a table's data ([`delimited_rows`]), so the per-column aggregation here and the
/// row-shaped reading beside it ([`super::table_rows`]) cannot drift on the delimiter, the
/// quoting or how many leading lines are a header.
async fn observe_delimited(
    bytes: &[u8],
    layout: &TableDataLayout,
    accumulators: &mut [(PlannedColumn, ColumnStatisticsAccumulator)],
    walked: &mut WalkedData,
) -> Result<(), String> {
    for record in delimited_rows(bytes, layout) {
        let record = record?;
        walked.walk_row(accumulators.len())?;
        for (column, accumulator) in accumulators.iter_mut() {
            accumulator.observe(cell_value(record.get(column.position), layout));
        }
        // The aggregation is the emulator's own work on the request's task, so it hands
        // the runtime back often enough for the run's budget, a cancellation and every
        // other request to land while it goes on.
        if walked.owes_yield() {
            ::tokio::task::yield_now().await;
        }
    }
    Ok(())
}

/// The DATA rows of one object of delimited text, in the order they are written: the leading
/// header lines the layout declares are not data and are not among them.
///
/// The reading is the `csv` crate's, over the delimiter and the quoting the table's serde
/// states: a Hive text table quotes nothing, so a quote there is an ordinary character,
/// while the open-CSV layout reads its stated quote character. Rows are read FLEXIBLY — a
/// row carrying fewer fields than the table declares columns leaves the rest holding no
/// value, which is how a text-format reader resolves a missing cell. A row that could not be
/// read is answered as the reason it could not, for the caller to report.
pub(super) fn delimited_rows<'bytes>(
    bytes: &'bytes [u8],
    layout: &TableDataLayout,
) -> impl Iterator<Item = Result<::csv::StringRecord, String>> + 'bytes {
    let header_lines = layout.header_lines;
    ::csv::ReaderBuilder::new()
        .delimiter(layout.delimiter)
        .has_headers(false)
        .flexible(true)
        .quoting(layout.quote.is_some())
        .quote(layout.quote.unwrap_or(b'"'))
        .from_reader(bytes)
        .into_records()
        .enumerate()
        .filter(move |(index, _)| *index >= header_lines)
        .map(|(_, record)| {
            record.map_err(|error| format!("a row could not be read: {error}"))
        })
}

/// Read the object as one JSON document per line, each member named by a column.
///
/// A JSON row addresses its cells by NAME, so the row's members are walked ONCE and each
/// is matched to the accumulator its folded name belongs to (`by_folded_name`, built once
/// per run) — rather than searching the row once per covered column.
async fn observe_json_lines(
    bytes: &[u8],
    layout: &TableDataLayout,
    accumulators: &mut [(PlannedColumn, ColumnStatisticsAccumulator)],
    by_folded_name: &HashMap<String, usize>,
    walked: &mut WalkedData,
) -> Result<(), String> {
    let text = ::std::str::from_utf8(bytes)
        .map_err(|error| format!("the object is not the UTF-8 text its layout states: {error}"))?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        walked.walk_row(accumulators.len())?;
        let document: ::serde_json::Value = ::serde_json::from_str(line)
            .map_err(|error| format!("a row is not the JSON document its layout states: {error}"))?;
        // Every covered column starts the row holding no value, and the row's own members
        // are what fill the ones it carries.
        let mut cells: Vec<Option<String>> = vec![None; accumulators.len()];
        if let Some(members) = document.as_object() {
            for (member, value) in members {
                if let Some(index) = by_folded_name.get(&fold_name(member)) {
                    if let Some(cell) = cells.get_mut(*index) {
                        *cell = json_cell(value);
                    }
                }
            }
        }
        for ((_, accumulator), cell) in accumulators.iter_mut().zip(cells.iter()) {
            accumulator.observe(cell_value(cell.as_deref(), layout));
        }
        if walked.owes_yield() {
            ::tokio::task::yield_now().await;
        }
    }
    Ok(())
}

/// The text a JSON member holds, or `None` for a member holding JSON's own null (and for a
/// member holding a whole array or object, which is not a cell of a column that HAS
/// column-level statistics).
pub(super) fn json_cell(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::Null | ::serde_json::Value::Array(_) | ::serde_json::Value::Object(_) => None,
        ::serde_json::Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// One cell as the accumulators take it: `None` for a cell the row did not carry, and for
/// one holding the layout's null marker.
pub(super) fn cell_value<'cell>(cell: Option<&'cell str>, layout: &TableDataLayout) -> Option<&'cell str> {
    cell.filter(|cell| *cell != layout.null_marker)
}

/// A stated single-character property as the byte a reader takes, or `None` for a value
/// that is not one byte — a property this reader cannot honour leaves the default in
/// place rather than failing the run.
fn single_byte(value: String) -> Option<u8> {
    let bytes = value.as_bytes();
    (bytes.len() == 1).then(|| bytes[0])
}

/// The value of one serde or table property, if the map states it.
fn property(properties: Option<&HashMap<String, String>>, name: &str) -> Option<String> {
    properties
        .and_then(|properties| properties.get(name))
        .cloned()
}

/// An Amazon S3 location split into the bucket and the key it names, or `None` for a value
/// that names no Amazon S3 bucket.
///
/// A table's location is a directory, so a value that does not end in the separator is
/// still a prefix of the objects under it — which is what the listing asks for. The same
/// reading resolves a job's `JobCommand$ScriptLocation`
/// ([`super::job_script`]), which names one object rather than a prefix, so the two cannot
/// read an `s3://` value two different ways.
pub fn split_s3_location(location: &str) -> Option<(String, String)> {
    let rest = location
        .strip_prefix("s3://")
        .or_else(|| location.strip_prefix("s3a://"))
        .or_else(|| location.strip_prefix("s3n://"))?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix),
        None => (rest, ""),
    };
    (!bucket.is_empty()).then(|| (bucket.to_string(), prefix.to_string()))
}

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// Shared with the CRAWL that walks a crawler's Amazon S3 targets
/// ([`super::crawl_source`]), so every read this service makes of the emulator's own object
/// store goes over ONE client configuration.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes IN PROCESS, so no network client is configured here and the `endpoint_url`
/// below is an address the in-process router reads rather than a host any resolver looks
/// up. `region` is the request's own signing region, so a run reads where the caller's own
/// S3 requests go; it is narrowed before it gets here ([`request_region`] admits only a
/// value shaped like a Region identifier).
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
pub(super) fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
