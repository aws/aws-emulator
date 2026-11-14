// SPDX-License-Identifier: CC0-1.0

//! The LISTING of a knowledge base's data sources: the order `ListDataSources` reports
//! them in, the page that listing hands out, and the cursor a page resumes from.
//!
//! A data source belongs to the knowledge base it was created under — the record names
//! it, and every modelled data-source operation addresses the pair — so one knowledge
//! base's listing never reports another's data source. The order is the one every other
//! listing of this service pages over: when the resource was created, ties broken by its
//! minted id, both of which never change, which is what lets a page resume exactly after
//! the one before it.

use super::*;

/// The most data sources one `ListDataSources` page reports when the request names no
/// `maxResults`.
///
/// The modelled `DataSourceSummaries` carries no `@length` bound and `maxResults` is
/// `range 1..1000`, so no bound is published for the page: the default is the whole of
/// what one knowledge base can hold ([`DATA_SOURCE_QUOTA`]), which is what a listing of
/// every data source of a knowledge base reports in one page.
pub const MAX_DATA_SOURCES_PER_PAGE: usize = DATA_SOURCE_QUOTA;

impl DataSourceRecord {
    /// This record as the modelled `DataSourceSummary` shape, which is what
    /// `ListDataSources` reports: every member comes from the same stored value the full
    /// projection reports it from.
    pub fn to_summary(&self) -> DataSourceSummary {
        DataSourceSummary {
            knowledge_base_id: self.knowledge_base_id.clone(),
            data_source_id: self.data_source_id.clone(),
            name: self.name.clone(),
            status: self.status.as_wire().to_string(),
            description: self.description.clone(),
            updated_at: to_date_time(self.updated_at),
        }
    }

    /// The total order `ListDataSources` pages over: when the data source was created,
    /// ties broken by its minted id.
    fn sort_key(&self) -> (u128, &str) {
        (epoch_nanos(self.created_at), self.data_source_id.as_str())
    }

    /// The `nextToken` a page ending at this data source hands out.
    pub fn page_cursor(&self) -> String {
        let (created_at_nanos, data_source_id) = self.sort_key();
        format!("{created_at_nanos}-{data_source_id}")
    }
}

/// A `ListDataSources` resume cursor, decoded back into the position it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSourcePageCursor {
    created_at_nanos: u128,
    data_source_id: String,
}

impl DataSourcePageCursor {
    /// Decode a `nextToken` a previous page handed out. `None` when the token was not
    /// produced by [`DataSourceRecord::page_cursor`] — only the one spelling that
    /// function writes names a position in this listing.
    pub fn parse(token: &str) -> Option<Self> {
        let (created_at_nanos, data_source_id) = parse_composite_cursor(token)?;
        Some(Self {
            created_at_nanos,
            data_source_id: data_source_id.to_string(),
        })
    }

    /// Whether `record` sorts after this cursor, i.e. belongs on the next page.
    fn precedes(&self, record: &DataSourceRecord) -> bool {
        (self.created_at_nanos, self.data_source_id.as_str()) < record.sort_key()
    }
}

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// One `ListDataSources` page: the data sources of this knowledge base after
    /// `cursor` in creation order, at most `page_size` of them, plus the token to resume
    /// from when more remain.
    ///
    /// Every data source of the knowledge base is reported, whatever its status —
    /// including one a delete has been accepted for, which reports the modelled
    /// `DELETING` until the removal the clock owes comes due, exactly as a read of it
    /// does — and no other knowledge base's. A knowledge base holding none reports an
    /// empty listing; an unknown one has nothing to enumerate, which the caller refuses
    /// as a missing resource before asking.
    pub fn page_of_data_sources(
        &self,
        knowledge_base_id: &str,
        cursor: Option<&DataSourcePageCursor>,
        page_size: usize,
    ) -> (Vec<DataSourceSummary>, Option<String>) {
        let mut ordered: Vec<&DataSourceRecord> = self
            .data_sources
            .values()
            .filter(|record| record.knowledge_base_id == knowledge_base_id)
            .filter(|record| cursor.is_none_or(|cursor| cursor.precedes(record)))
            .collect();
        ordered.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        let page: Vec<&DataSourceRecord> = ordered.iter().take(page_size.max(1)).copied().collect();
        let next_token = (ordered.len() > page.len())
            .then(|| page.last().map(|record| record.page_cursor()))
            .flatten();
        (
            page.iter().map(|record| record.to_summary()).collect(),
            next_token,
        )
    }
}
