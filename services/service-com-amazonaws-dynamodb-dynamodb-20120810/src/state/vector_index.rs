// SPDX-License-Identifier: CC0-1.0

//! One VECTOR INDEX's own behaviour: the status it reports at an instant, the phases its
//! build passes through, and what accepting its deletion leaves behind.
//!
//! The declaration it is about lives beside this file in [`super`] (the stored shape a
//! table carries), as does the DECLARATION a request states one with
//! (`super::vector_index_declarations`); what is here is the lifecycle those two meet in.
//! `super` re-exports it, so every caller still names it `crate::state::…`.

use super::*;

impl VectorIndex {
    /// The status at `now`, derived from the stored instants, by the same rule a
    /// secondary index follows.
    ///
    /// An index whose delete has been accepted reports `DELETING` — "the index is being
    /// deleted" (`VectorIndexDescription$IndexStatus`) — for as long as the table still
    /// carries it, whichever status it was in when the delete arrived: the removal is the
    /// last thing that happens to it, so no build it had in flight can take it back to
    /// `ACTIVE` ([`Self::begin_deletion`]).
    pub fn status(&self, now: SystemTime) -> IndexStatus {
        if self.being_deleted() {
            IndexStatus::Deleting
        } else if now >= self.active_at {
            IndexStatus::Active
        } else {
            self.transient_status
        }
    }

    /// Whether this index's creation is still in its RESOURCE-ALLOCATION phase at `now`:
    /// it is being created, and the clock has not yet made the end of that phase due
    /// ([`Self::allocated_at`]).
    ///
    /// What a caller may do to an index being BUILT follows which of the two phases its
    /// build is in: the resources are allocated before anything is read, and an index whose
    /// allocation is still running is not yet a resource a delete can take away — so that
    /// delete is refused, while an index that has reached its backfill, or settled, is
    /// deleted.
    pub fn in_resource_allocation(&self, now: SystemTime) -> bool {
        self.status(now) == IndexStatus::Creating && now < self.allocated_at
    }

    /// Accept this index's DELETE, owing its removal at `due_at`.
    ///
    /// Removing an index is asynchronous work on the table: the call that asks for it
    /// answers a description that still carries the index, in the `DELETING` status that
    /// says what is happening to it, and the entry disappears only once the clock makes
    /// the removal due ([`Table::apply_due_index_removals`]). A caller polling the index
    /// therefore sees `DELETING` and then its absence, which is what tells "delete
    /// requested" apart from "delete finished".
    pub fn begin_deletion(&mut self, due_at: SystemTime) {
        self.deleting_at = Some(due_at);
    }

    /// Whether this index's delete has been accepted, so the table is only still carrying
    /// it until the removal falls due.
    ///
    /// An index on its way out is no longer one of the indexes the TABLE is held to — it
    /// takes no slot of the per-table quota, spends no projection budget and keeps no
    /// attribute definition alive — and it is not addressable either: only an `ACTIVE`
    /// index serves a search.
    pub fn being_deleted(&self) -> bool {
        self.deleting_at.is_some()
    }

    /// Whether the clock has already made this index's removal due at `now`.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.deleting_at.is_some_and(|due_at| now >= due_at)
    }

    /// Every attribute of the TABLE this index consumes: the vector attribute, and each
    /// attribute its search schema names.
    ///
    /// This is what makes an `AttributeDefinitions` entry used by a vector index, and what
    /// the index stores for an item beside whatever its `Projection` adds.
    pub fn consumed_attributes(&self) -> impl Iterator<Item = &str> {
        ::std::iter::once(self.vector_attribute.attribute_name.as_str()).chain(
            self.search_schema
                .iter()
                .map(|element| element.attribute_name.as_str()),
        )
    }

    /// Whether `item` reaches this index: it must carry the vector attribute, and the
    /// value of every attribute the search schema partitions the index by
    /// ([`SEARCH_SCHEMA_PARTITION_KEY`]). An `INLINE_FILTER` attribute is optional, so an
    /// item that leaves one out is still indexed.
    pub fn covers(&self, item: &Item) -> bool {
        item.contains_key(&self.vector_attribute.attribute_name)
            && self
                .search_schema
                .iter()
                .filter(|element| element.search_schema_element_type == SEARCH_SCHEMA_PARTITION_KEY)
                .all(|element| item.contains_key(&element.attribute_name))
    }

    /// Which of a covered item's attributes this index STORES, or `None` when it stores
    /// every one of them.
    ///
    /// An index holds "attributes that are copied (projected) from the table into the
    /// vector index" (`VectorIndex$Projection`): the attributes it consumes (its vector
    /// attribute and its search schema), the table's own key attributes, and whatever its
    /// `Projection` adds by the same rule a secondary index is projected by
    /// ([`index_projects`]). `ALL` stores the whole item, which is `None` here.
    pub fn stored_attributes(
        &self,
        table_key_schema: &[KeySchemaElement],
    ) -> Option<BTreeSet<String>> {
        if projects_all(&self.projection) {
            return None;
        }
        let mut names: BTreeSet<String> = self
            .consumed_attributes()
            .map(ToString::to_string)
            .collect();
        // Which of the TABLE's attributes a non-`ALL` projection adds is
        // [`index_projects`]'s rule to state, so the candidates are filtered through it
        // rather than re-derived here.
        names.extend(
            table_key_schema
                .iter()
                .map(|element| element.attribute_name.as_str())
                .chain(
                    self.projection
                        .non_key_attributes
                        .iter()
                        .flatten()
                        .map(String::as_str),
                )
                .filter(|name| index_projects(&[], &self.projection, table_key_schema, name))
                .map(ToString::to_string),
        );
        Some(names)
    }

    /// The entry this index holds for one covered item: just the attributes it stores
    /// ([`Self::stored_attributes`]), which is what a write of the entry meters and what
    /// the search condition filters on.
    ///
    /// An index holds "attributes that are copied (projected) from the table into the vector
    /// index" (`VectorIndex$Projection`) and, beside them, the vector attribute it finds an
    /// item by and the attributes its search schema filters on
    /// ([`Self::consumed_attributes`]) — so one entry serves every question about what the
    /// index HOLDS for an item: what a write of it meters
    /// ([`Table::vector_write_request_bytes`]), what a condition reads, and what a search
    /// READS of it — "only attributes projected into the vector index can be retrieved"
    /// (`SearchVectors$ProjectionExpression`), and the vector attribute is among them, since
    /// it is the index's own key attribute and "the primary key attributes and index key
    /// attributes … are automatically projected" (`Projection`). What a search HANDS BACK is
    /// narrower still — a result carries "the projected attributes of the item returned by
    /// the vector search" (`SearchResultItem$Item`), which the vector the index is built on
    /// is not one of — and that is the search's own to decide, not this entry's.
    ///
    /// The vector itself is held as the index holds it rather than as the item does
    /// ([`stored_vector`]): "each element is a 32-bit IEEE-754 floating point number"
    /// (`SearchVectors$SearchVector`), so a coordinate no such float represents exactly is
    /// held — and reported, and compared, and metered — as the float it rounds to.
    pub fn index_entry(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> Item {
        let mut entry: Item = match self.stored_attributes(table_key_schema) {
            None => item.clone(),
            Some(stored) => item
                .iter()
                .filter(|(name, _)| stored.contains(name.as_str()))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        };
        if let Some(value) = entry.get_mut(&self.vector_attribute.attribute_name) {
            if let Some(held) = stored_vector(value) {
                *value = held;
            }
        }
        entry
    }

    /// The bytes this index stores for one covered item: the size of the entry it holds
    /// for it, so what the index is sized by and what a write of it meters are the same
    /// projection.
    pub(super) fn projected_size(&self, item: &Item, table_key_schema: &[KeySchemaElement]) -> i64 {
        item_size(&self.index_entry(item, table_key_schema))
    }

    /// Project this index into the `VectorIndexDescription` every table description
    /// reports it as. `ItemCount` and `IndexSizeBytes` are the metered figures of the
    /// last refresh, never the live projection.
    pub(super) fn describe(&self, now: SystemTime) -> VectorIndexDescription {
        VectorIndexDescription {
            index_name: Some(self.name.clone()),
            index_arn: Some(self.arn.clone()),
            vector_attribute: Some(self.vector_attribute.clone()),
            search_schema: (!self.search_schema.is_empty()).then(|| self.search_schema.clone()),
            projection: Some(self.projection.clone()),
            dimensions: Some(self.dimensions),
            distance_function: Some(self.distance_function.clone()),
            index_status: Some(self.status(now).as_str().to_string()),
            // "Specifies whether the index is currently backfilling. During backfill,
            // SearchVectors operations might return incomplete results"
            // (`VectorIndexDescription$Backfilling`): true while the items the table
            // already held when this index was added are still being read
            // ([`IndexBackfill`]), and absent for an index defined with the table.
            backfilling: self.backfill.backfilling(now),
            item_count: Some(self.reported_counters.item_count),
            index_size_bytes: Some(self.reported_counters.size_bytes),
        }
    }
}
