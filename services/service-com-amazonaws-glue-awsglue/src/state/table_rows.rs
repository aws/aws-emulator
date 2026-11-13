// SPDX-License-Identifier: CC0-1.0

//! Reading the ROWS of a Data Catalog table: the objects under its
//! `StorageDescriptor$Location`, laid out the way its `StorageDescriptor$SerdeInfo` says,
//! resolved into one cell per column the table declares.
//!
//! A table's data lives in the emulator's OWN Amazon S3 and is read through the framework's
//! in-process callback, exactly as a column statistics task run reads it
//! ([`super::column_statistics_source`]): that module owns the ONE client configuration,
//! the ONE object walk and the ONE layout reading this service makes of a table's data, and
//! this is the row-shaped reading over them, for the work that needs the rows themselves
//! rather than per-column aggregates — a data quality ruleset evaluation run, which
//! evaluates its rules against the table's real data.
//!
//! HOW MUCH IS READ IS BOUNDED by the same bounds the statistics run is bounded by, because
//! the bound is on what this emulator reads at once rather than on which run asked: the
//! objects and bytes the walk admits, the rows and cells one read resolves, the wall clock
//! one read is given and how many reads run at a time. Data past any of them is data this
//! read could not take, which is a read that answers WHY rather than one that answers part
//! of the rows.

use super::*;

/// Where one table's data is and how its objects are laid out, plus the columns the stored
/// table declares.
///
/// Built from the STORED table under the store's guard and then carried out with no guard
/// held, so a read covers the table as it stood when it started — the same split a
/// statistics run's plan makes ([`ColumnStatisticsRunPlan`]).
#[derive(Debug, Clone)]
pub struct TableDataPlan {
    /// The columns the table declares, in the order it declares them, which is the order a
    /// row of delimited text carries their cells in.
    pub columns: Vec<String>,
    /// `StorageDescriptor$Location`: where the table's data lives.
    pub location: Option<String>,
    /// How the objects there are laid out.
    pub layout: TableDataLayout,
}

impl TableDataPlan {
    /// The plan of a read of `table`.
    pub fn of(table: &TableEntry) -> Self {
        Self {
            columns: table_column_names(table),
            location: table
                .storage_descriptor
                .as_ref()
                .and_then(|descriptor| descriptor.location.clone()),
            layout: TableDataLayout::of(table),
        }
    }
}

/// The rows one read of a table's data resolved.
#[derive(Debug, Clone, Default)]
pub struct TableRows {
    /// The columns the stored table declared when the read started.
    pub columns: Vec<String>,
    /// One entry per row read, each holding one cell per declared column: `None` for a cell
    /// the row did not carry and for one holding the layout's null marker, which is what a
    /// text-format reader resolves a missing or NULL cell to.
    pub rows: Vec<Vec<Option<String>>>,
}

impl TableRows {
    /// Which cell of a row belongs to the column `name`, or `None` when the table declares
    /// no such column. Matched folded, the way the catalog matches a column name.
    pub fn column_position(&self, name: &str) -> Option<usize> {
        let wanted = fold_name(name);
        self.columns
            .iter()
            .position(|column| fold_name(column) == wanted)
    }

    /// The cells of one column, in row order, or `None` when the table declares no such
    /// column.
    pub fn column_cells(&self, name: &str) -> Option<Vec<Option<&str>>> {
        let position = self.column_position(name)?;
        Some(
            self.rows
                .iter()
                .map(|row| row.get(position).and_then(|cell| cell.as_deref()))
                .collect(),
        )
    }
}

/// Read every row of the table `plan` describes out of the emulator's own Amazon S3.
///
/// Awaited with NO store guard held. `Err` is why the data could not be read, for the
/// caller to report as the reason the work it was read for produced nothing.
pub async fn read_table_rows(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &TableDataPlan,
) -> Result<TableRows, String> {
    let Some(location) = plan
        .location
        .as_deref()
        .map(str::trim)
        .filter(|location| !location.is_empty())
    else {
        return Err("the table states no storage location, so it points at no data".to_string());
    };
    if !plan.layout.text_input_format {
        return Err("the table's input format is not one whose objects are lines of text".to_string());
    }
    let Some((bucket, prefix)) = split_s3_location(location) else {
        return Err("the table's location does not name an Amazon S3 bucket".to_string());
    };

    // How many reads run at once is bounded, and so is the wait for a permit, so a caller
    // answers either way rather than queueing without end.
    let permit = match ::tokio::time::timeout(
        COLUMN_STATISTICS_PERMIT_WAIT,
        COLUMN_STATISTICS_RUN_PERMITS.acquire(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        _ => return Err("no free reader took the table's data".to_string()),
    };
    let client = s3_client(ctx, region);
    let read = ::tokio::time::timeout(
        COLUMN_STATISTICS_RUN_BUDGET,
        collect_rows(&client, &bucket, &prefix, plan),
    )
    .await;
    drop(permit);

    match read {
        Ok(rows) => rows,
        Err(_elapsed) => {
            Err("the table's data was not read inside the time one read is given".to_string())
        }
    }
}

/// Read the objects under `prefix` into rows, or answer why the data could not be read.
async fn collect_rows(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    prefix: &str,
    plan: &TableDataPlan,
) -> Result<TableRows, String> {
    let objects = list_objects(client, bucket, prefix).await?;
    if objects.is_empty() {
        return Err("the table's location holds no object".to_string());
    }
    let mut rows = TableRows {
        columns: plan.columns.clone(),
        rows: Vec::new(),
    };
    // Which cell a NAMED member of a JSON row belongs to, folded once per read rather than
    // once per cell.
    let by_folded_name: HashMap<String, usize> = plan
        .columns
        .iter()
        .enumerate()
        .map(|(position, column)| (fold_name(column), position))
        .collect();
    for key in objects {
        let bytes = read_object(client, bucket, &key).await?;
        if plan.layout.json {
            read_json_lines(&bytes, plan, &by_folded_name, &mut rows).await?;
        } else {
            read_delimited(&bytes, plan, &mut rows).await?;
        }
    }
    Ok(rows)
}

/// Whether taking one more row of `columns` cells would carry this read past either bound,
/// and the reason it would.
fn admits_another_row(rows: &TableRows, columns: usize) -> Result<(), String> {
    if rows.rows.len() >= MAX_COLUMN_STATISTICS_ROWS {
        return Err(format!(
            "the location holds more than the {MAX_COLUMN_STATISTICS_ROWS} rows one read \
             resolves"
        ));
    }
    if rows.rows.len().saturating_mul(columns.max(1)) >= MAX_COLUMN_STATISTICS_CELLS {
        return Err(format!(
            "the columns of those rows are more than the {MAX_COLUMN_STATISTICS_CELLS} cells \
             one read resolves"
        ));
    }
    Ok(())
}

/// Read the object as rows of delimited text, one field per declared column.
///
/// The reader and the header-line handling are the statistics module's
/// ([`delimited_rows`]), the ONE delimited reading this service makes of a table's data, so
/// the rows resolved here and the cells a statistics run aggregates cannot drift on the
/// delimiter, the quoting or how many leading lines are a header.
async fn read_delimited(
    bytes: &[u8],
    plan: &TableDataPlan,
    rows: &mut TableRows,
) -> Result<(), String> {
    for record in delimited_rows(bytes, &plan.layout) {
        let record = record?;
        admits_another_row(rows, plan.columns.len())?;
        let row: Vec<Option<String>> = (0..plan.columns.len())
            .map(|position| {
                cell_value(record.get(position), &plan.layout).map(str::to_string)
            })
            .collect();
        rows.rows.push(row);
        // The reading is this emulator's own work on the request's task, so it hands the
        // runtime back often enough for the read's budget and every other request to land.
        if rows.rows.len().is_multiple_of(YIELD_EVERY_ROWS) {
            ::tokio::task::yield_now().await;
        }
    }
    Ok(())
}

/// Read the object as one JSON document per line, each member named by a column.
async fn read_json_lines(
    bytes: &[u8],
    plan: &TableDataPlan,
    by_folded_name: &HashMap<String, usize>,
    rows: &mut TableRows,
) -> Result<(), String> {
    let text = ::std::str::from_utf8(bytes)
        .map_err(|error| format!("the object is not the UTF-8 text its layout states: {error}"))?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        admits_another_row(rows, plan.columns.len())?;
        let document: ::serde_json::Value = ::serde_json::from_str(line)
            .map_err(|error| format!("a row is not the JSON document its layout states: {error}"))?;
        // Every declared column starts the row holding no value, and the row's own members
        // are what fill the ones it carries.
        let mut row: Vec<Option<String>> = vec![None; plan.columns.len()];
        if let Some(members) = document.as_object() {
            for (member, value) in members {
                if let Some(position) = by_folded_name.get(&fold_name(member)) {
                    if let Some(cell) = row.get_mut(*position) {
                        *cell = json_cell(value)
                            .and_then(|held| {
                                cell_value(Some(held.as_str()), &plan.layout).map(str::to_string)
                            });
                    }
                }
            }
        }
        rows.rows.push(row);
        if rows.rows.len().is_multiple_of(YIELD_EVERY_ROWS) {
            ::tokio::task::yield_now().await;
        }
    }
    Ok(())
}
