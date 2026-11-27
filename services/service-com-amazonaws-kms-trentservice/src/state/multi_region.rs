// SPDX-License-Identifier: CC0-1.0

//! Multi-Region key sets: which Region of one set holds its primary key, which hold
//! replicas, and the move of that designation `UpdatePrimaryRegion` starts.
//!
//! One concern of [`super`]'s store, in its own file: the records and helpers here are
//! reached as `crate::state::…` exactly as they were before, because the parent
//! re-exports them.

use std::collections::BTreeSet;

use super::*;
use crate::ops::types::{MultiRegionConfiguration, MultiRegionKey};

/// Which Region of one multi-Region key set holds its PRIMARY key, and which
/// Regions hold its replica keys.
///
/// Every key of a multi-Region set shares one key id — "these KMS keys have the
/// same key ID, key material, and other metadata"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — and this store holds
/// one record per key id, so the set is ONE record and what distinguishes its
/// members is the Region each of them lives in. Exactly one Region holds the
/// primary key at any instant; every other Region named here holds a replica of
/// it, and which is which is what `MultiRegionConfiguration` reports.
#[derive(Debug, Clone)]
pub struct MultiRegionTopology {
    /// The Region holding the set's primary key once every move of the designation
    /// the clock owes has taken effect: the Region a `MultiRegion` create minted it
    /// in, until `UpdatePrimaryRegion` moves the designation elsewhere. A reader asks
    /// [`Self::primary_region`] instead, which is the designation as it stands NOW.
    settled_primary_region: String,
    /// The Regions holding a replica key of the set once every move of the
    /// designation the clock owes has taken effect. Sorted, so a reader reports the
    /// replicas in a stable order across calls.
    settled_replica_regions: BTreeSet<String>,
    /// The move of the designation `UpdatePrimaryRegion` started and the clock has
    /// not yet made due, or `None` when the set has no move outstanding — see
    /// [`PendingPromotion`].
    promotion: Option<PendingPromotion>,
}

impl MultiRegionTopology {
    /// A freshly minted multi-Region primary key's topology: primary in the Region
    /// it was created in, with no replica anywhere until one is created in another
    /// Region.
    pub fn primary_in(region: String) -> Self {
        Self {
            settled_primary_region: region,
            settled_replica_regions: BTreeSet::new(),
            promotion: None,
        }
    }

    /// Record that `region` now holds a replica key of this set.
    pub fn add_replica_region(&mut self, region: String) {
        self.settled_replica_regions.insert(region);
    }

    /// Start moving the designation to the replica key in `region`, due at `due`,
    /// answering whether that Region held a replica to promote at all.
    ///
    /// "This operation changes the replica key in the specified Region to a primary
    /// key and changes the former primary key to a replica key"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`), so the two moves
    /// are one step: the set never has two primary keys, nor none. A Region holding
    /// no replica of the set — including the one that already holds the primary key
    /// — has nothing to promote and leaves the topology exactly as it was.
    ///
    /// The move takes effect at `due` rather than at once, because until then "the
    /// process of updating the primary Region might still be in progress" (same
    /// doc): the designation this call moves and the transient key state that move
    /// puts the two keys in are the SAME record, so neither is ever observable
    /// without the other.
    pub fn promote(&mut self, region: &str, due: SystemTime) -> bool {
        self.settle_due_promotion();
        if !self.settled_replica_regions.contains(region) {
            return false;
        }
        self.promotion = Some(PendingPromotion {
            region: region.to_string(),
            due,
        });
        true
    }

    /// Whether the clock still owes this set the move of its designation a call has
    /// started — the set is transiently busy, which is what refuses the management
    /// operations of its keys.
    pub fn promotion_in_flight(&self) -> bool {
        self.promotion
            .as_ref()
            .is_some_and(|promotion| SystemTime::now() < promotion.due)
    }

    /// Whether the key `region` holds is one of the two the move in flight goes
    /// between — the old primary key and the new one, which are the keys that carry
    /// the transient [`KEY_STATE_UPDATING`] state while it is in flight.
    pub fn is_moving_through(&self, region: &str) -> bool {
        self.is_moving_through_at(region, SystemTime::now())
    }

    /// The same, as of ONE instant ([`crate::state::Cmk::metadata_at`]).
    pub fn is_moving_through_at(&self, region: &str, now: SystemTime) -> bool {
        self.promotion.as_ref().is_some_and(|promotion| {
            now < promotion.due
                && (promotion.region == region || self.settled_primary_region == region)
        })
    }

    /// The Region holding this set's primary key NOW: the Region a move the clock has
    /// already made due promoted, and otherwise the one the designation rests in. A
    /// move still in flight has not happened yet, so it moves nothing a reader sees.
    pub fn primary_region(&self) -> &str {
        self.primary_region_at(SystemTime::now())
    }

    /// The same, as of ONE instant ([`crate::state::Cmk::metadata_at`]).
    pub fn primary_region_at(&self, now: SystemTime) -> &str {
        match self.due_promotion_at(now) {
            Some(promoted) => promoted,
            None => &self.settled_primary_region,
        }
    }

    /// The Regions holding a replica key of this set as of `now` — every Region of the
    /// set but the one [`Self::primary_region_at`] reports, in the sorted order the
    /// backing set keeps them in.
    fn replica_regions_at(&self, now: SystemTime) -> BTreeSet<&str> {
        match self.due_promotion_at(now) {
            Some(promoted) => self
                .settled_replica_regions
                .iter()
                .map(String::as_str)
                .filter(|region| *region != promoted)
                .chain(std::iter::once(self.settled_primary_region.as_str()))
                .collect(),
            None => self
                .settled_replica_regions
                .iter()
                .map(String::as_str)
                .collect(),
        }
    }

    /// The Region a move the clock has ALREADY made due promoted, which is the
    /// designation this topology has not written down yet.
    fn due_promotion(&self) -> Option<&str> {
        self.due_promotion_at(SystemTime::now())
    }

    /// The same, as of ONE instant ([`crate::state::Cmk::metadata_at`]).
    fn due_promotion_at(&self, now: SystemTime) -> Option<&str> {
        self.promotion
            .as_ref()
            .filter(|promotion| now >= promotion.due)
            .map(|promotion| promotion.region.as_str())
    }

    /// Write a move the clock has already made due into the designation, so a move
    /// started after it moves the set on from where that one left it. What a reader
    /// sees is unchanged: this is the same designation [`Self::primary_region`]
    /// already reports.
    fn settle_due_promotion(&mut self) {
        let Some(promoted) = self.due_promotion().map(str::to_string) else {
            return;
        };
        self.settled_replica_regions.remove(&promoted);
        let former_primary = std::mem::replace(&mut self.settled_primary_region, promoted);
        self.settled_replica_regions.insert(former_primary);
        self.promotion = None;
    }

    /// This set as the `MultiRegionConfiguration` of the key `region`'s endpoint
    /// serves: that key is the set's primary exactly when the primary Region is its
    /// own, and `ReplicaKeys` names every Region holding a replica, each by the key
    /// ARN that Region's endpoint serves it under.
    pub fn configuration(&self, key_id: &str, region: &str) -> MultiRegionConfiguration {
        self.configuration_at(key_id, region, SystemTime::now())
    }

    /// The same, as of ONE instant ([`crate::state::Cmk::metadata_at`]).
    pub fn configuration_at(
        &self,
        key_id: &str,
        region: &str,
        now: SystemTime,
    ) -> MultiRegionConfiguration {
        let primary_region = self.primary_region_at(now);
        let key_type = if primary_region == region {
            MULTI_REGION_KEY_TYPE_PRIMARY
        } else {
            MULTI_REGION_KEY_TYPE_REPLICA
        };
        MultiRegionConfiguration {
            multi_region_key_type: Some(key_type.to_string()),
            primary_key: Some(multi_region_member(key_id, primary_region)),
            replica_keys: Some(
                self.replica_regions_at(now)
                    .into_iter()
                    .map(|replica| multi_region_member(key_id, replica))
                    .collect(),
            ),
        }
    }
}

/// The move of one multi-Region key set's primary-key designation that an
/// `UpdatePrimaryRegion` call started and the clock has not yet made due.
///
/// "Even after this operation completes, the process of updating the primary Region
/// might still be in progress for a few more seconds. ... The old and new primary
/// keys have a transient key state of Updating. The original key state is restored
/// when the update is complete"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — so ONE record carries
/// both halves of the move: which Region the designation is going to, and the instant
/// it arrives there, which is also the instant the transient state it put the two keys
/// in is over.
#[derive(Debug, Clone)]
struct PendingPromotion {
    /// The Region whose replica key the move makes this set's primary key.
    region: String,
    /// When the move is due — [`PRIMARY_REGION_UPDATE_SPAN`] after the call that
    /// started it. Nothing has to be written when it passes: a reader derives the
    /// designation and the key state from it.
    due: SystemTime,
}

/// One key of a multi-Region set, as `MultiRegionConfiguration` names it: the
/// Region it lives in, and the key ARN that Region's endpoint serves it under.
fn multi_region_member(key_id: &str, region: &str) -> MultiRegionKey {
    MultiRegionKey {
        arn: Some(key_arn(region, key_id)),
        region: Some(region.to_string()),
    }
}
