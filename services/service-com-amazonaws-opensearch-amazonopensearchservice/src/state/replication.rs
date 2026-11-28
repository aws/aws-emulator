// SPDX-License-Identifier: CC0-1.0

//! A domain's CROSS-CLUSTER REPLICATION arrangement, as this emulator persists it: which peer
//! domain it replicates with, which role it plays in that relationship, the state the
//! relationship is in, and the one wording this service refuses a call on a domain that
//! replicates with nobody.
//!
//! The vocabulary is the MODEL's: a domain plays `ReplicationRole`'s
//! [`REPLICATION_ROLE_PRIMARY`] or [`REPLICATION_ROLE_SECONDARY`], and one relationship is in
//! one of `ReplicationStatus`'s states
//! (`smithy_query(query="shape",shape="ReplicationPeer,ReplicationRole,ReplicationStatus")`).
//! Nothing beyond what those shapes carry is invented.
//!
//! The record lives ON the [`Domain`] it belongs to, for the reason its VPC endpoint
//! authorizations do: an arrangement is a property of the domain, so releasing the domain
//! releases it. BOTH sides of one relationship are written by the one call that changes it
//! it ([`crate::ops::add_replication_peer`]), so a read of either domain reports the same
//! arrangement.
//!
//! Every change is ASYNCHRONOUS — "replication is established asynchronously; use the
//! `DescribeDomainReplication` operation to check whether the relationship has reached the
//! `ACTIVE` state" — so a call records the transient status the model names for it and the
//! instant the clock owes the next one ([`PendingMove`]), and [`DomainReplication::settle`]
//! materialises it. Nothing sleeps.

use super::*;

/// The payloads this service refuses a call on a replication arrangement with, one per
/// condition the five calls refuse.
mod refusals;
pub use refusals::*;

/// The role of the domain a relationship replicates FROM: the side that takes writes.
pub const REPLICATION_ROLE_PRIMARY: &str = "PRIMARY";

/// The role of the domain a relationship replicates TO — "receives a copy of the primary's
/// indexes and serves read requests".
pub const REPLICATION_ROLE_SECONDARY: &str = "SECONDARY";

/// `ReplicationStatus$PENDING_SETUP`: "replication is being established".
pub const REPLICATION_STATUS_PENDING_SETUP: &str = "PENDING_SETUP";

/// `ReplicationStatus$ACTIVE`: "replication is running".
pub const REPLICATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `ReplicationStatus$SWITCHING_OVER`: "a planned promotion is in progress".
pub const REPLICATION_STATUS_SWITCHING_OVER: &str = "SWITCHING_OVER";

/// `ReplicationStatus$FAILING_OVER`: "an unplanned promotion is in progress".
pub const REPLICATION_STATUS_FAILING_OVER: &str = "FAILING_OVER";

/// `ReplicationStatus$PENDING_REMOVAL`: "replication is being stopped".
pub const REPLICATION_STATUS_PENDING_REMOVAL: &str = "PENDING_REMOVAL";

/// `ReplicationStatus$EJECTED`: "a failover promoted the other domain. The former primary
/// keeps the data it had when it was ejected and stays read-only until you remove the peer".
pub const REPLICATION_STATUS_EJECTED: &str = "EJECTED";

/// How many peers one domain replicates with: "a domain can have one peer"
/// (`smithy_query(query="doc",shape="AddReplicationPeer")`), so a second relationship is
/// refused rather than stored.
pub const PEER_QUOTA: usize = 1;

/// How long a change to a replication relationship takes before the clock owes its outcome.
///
/// [`DOMAIN_WORK`], the span this control plane gives work on a domain, for the reason that
/// span states: establishing replication, stopping it and promoting a domain are each work on
/// the two clusters, and each leaves the relationship in a transient status the model names
/// and a client POLLS for ("use the `DescribeDomainReplication` operation to check whether
/// the relationship has reached the `ACTIVE` state"). A span of a few hundred milliseconds is
/// one such a poll steps straight over. AWS takes many minutes.
pub const REPLICATION_CHANGE: Duration = DOMAIN_WORK;

/// The other side of `role`: the role the peer of a domain playing `role` plays.
///
/// A relationship has exactly two sides, so this is a total function over the two roles
/// above; anything else is a role this service never writes and is paired with the primary,
/// the side the peer of a secondary domain is on.
#[must_use]
pub fn counterpart_role(role: &str) -> &'static str {
    if role == REPLICATION_ROLE_PRIMARY {
        REPLICATION_ROLE_SECONDARY
    } else {
        REPLICATION_ROLE_PRIMARY
    }
}

/// What the clock owes one relationship: the status it reaches, the role the HOLDING domain
/// takes with it, and whether the relationship ends there.
#[derive(Debug, Clone)]
pub struct PendingMove {
    /// The instant the move falls due. Stored rather than slept on: whichever later call
    /// reads the record materialises it ([`DomainReplication::settle`]).
    pub due: SystemTime,
    /// The status the relationship is in once the move has happened.
    pub status: String,
    /// The role the holding domain plays once it has, for a move that changes the roles — a
    /// promotion — and `None` for one that does not.
    pub role: Option<String>,
    /// Whether the relationship is GONE once the move has happened: "a relationship that is
    /// removed successfully no longer appears in `Peers`".
    pub ends: bool,
}

/// One peer of a domain's replication arrangement, and the state of that one relationship —
/// the `ReplicationPeer` a read reports.
#[derive(Debug, Clone)]
pub struct PeerRelationship {
    /// The ARN of the peer domain — a domain of this store, which is what the call that
    /// establishes the relationship resolves before writing either side of it.
    pub peer_domain_arn: String,
    /// The role the PEER plays, which is the other side of the role the holding domain plays
    /// ([`counterpart_role`]).
    pub peer_role: String,
    /// `ReplicationPeer$Status`: the state of this one relationship.
    pub status: String,
    /// `ReplicationPeer$StatusMessage`: "an explanation of why the relationship is in its
    /// current state, WHEN MORE DETAIL IS AVAILABLE" — so it is absent unless this service
    /// has something to say beyond the status itself.
    pub status_message: Option<String>,
    /// What the clock owes this relationship, and `None` once it is in a settled state.
    pub pending: Option<PendingMove>,
}

impl PeerRelationship {
    /// This relationship as the modelled `ReplicationPeer` a read reports.
    #[must_use]
    pub fn to_peer(&self) -> ReplicationPeer {
        ReplicationPeer {
            peer_domain_arn: Some(self.peer_domain_arn.clone()),
            peer_role: Some(self.peer_role.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
        }
    }
}

/// A domain's whole replication arrangement: the peer it replicates with, and the role it
/// plays with it.
#[derive(Debug, Clone, Default)]
pub struct DomainReplication {
    /// The role this domain plays, and `None` while it replicates with nobody — which is
    /// why a read of such a domain reports no role at all.
    pub role: Option<String>,
    /// The relationships, keyed by the peer domain's ARN: a domain holds at most one
    /// relationship per peer, so the ARN IS the key, and [`PEER_QUOTA`] bounds the set.
    /// Iterating a `BTreeMap` is order-stable, which is what lets two reads report one
    /// domain's peers in the same order.
    pub peers: BTreeMap<String, PeerRelationship>,
}

impl DomainReplication {
    /// Whether this domain replicates with anybody at all — the condition the replication
    /// calls that act on an existing relationship refuse on ([`domain_not_replicating`]).
    #[must_use]
    pub fn replicating(&self) -> bool {
        !self.peers.is_empty()
    }

    /// Whether this domain replicates with the peer `peer_domain_arn` names.
    #[must_use]
    pub fn replicates_with(&self, peer_domain_arn: &str) -> bool {
        self.peers.contains_key(peer_domain_arn)
    }

    /// The relationship with the peer `peer_domain_arn` names, if this domain holds one.
    #[must_use]
    pub fn peer(&self, peer_domain_arn: &str) -> Option<&PeerRelationship> {
        self.peers.get(peer_domain_arn)
    }

    /// Record a relationship with the peer `peer_domain_arn` names, this domain playing
    /// `role` and the peer the other side of it.
    ///
    /// The relationship is [`REPLICATION_STATUS_PENDING_SETUP`] — "this is `PENDING_SETUP`
    /// immediately after the request is accepted" — and the clock owes it
    /// [`REPLICATION_STATUS_ACTIVE`] one [`REPLICATION_CHANGE`] later.
    ///
    /// Idempotent in the peer: a repeat for one peer overwrites that peer's entry rather
    /// than holding two relationships with one domain.
    pub fn establish(&mut self, peer_domain_arn: &str, role: &str, now: SystemTime) -> String {
        self.role = Some(role.to_string());
        self.peers.insert(
            peer_domain_arn.to_string(),
            PeerRelationship {
                peer_domain_arn: peer_domain_arn.to_string(),
                peer_role: counterpart_role(role).to_string(),
                status: REPLICATION_STATUS_PENDING_SETUP.to_string(),
                status_message: None,
                pending: Some(PendingMove {
                    due: now + REPLICATION_CHANGE,
                    status: REPLICATION_STATUS_ACTIVE.to_string(),
                    role: None,
                    ends: false,
                }),
            },
        );
        REPLICATION_STATUS_PENDING_SETUP.to_string()
    }

    /// Start stopping the relationship with the peer `peer_domain_arn` names, and answer the
    /// status it is then in — or `None` when this domain holds no such relationship.
    ///
    /// The relationship goes [`REPLICATION_STATUS_PENDING_REMOVAL`] — "this is
    /// `PENDING_REMOVAL` while replication is being stopped" — and the clock owes its REMOVAL
    /// one [`REPLICATION_CHANGE`] later, because "a relationship that is removed successfully
    /// no longer appears in `Peers`".
    pub fn begin_removal(&mut self, peer_domain_arn: &str, now: SystemTime) -> Option<String> {
        let relationship = self.peers.get_mut(peer_domain_arn)?;
        relationship.status = REPLICATION_STATUS_PENDING_REMOVAL.to_string();
        relationship.status_message = None;
        relationship.pending = Some(PendingMove {
            due: now + REPLICATION_CHANGE,
            status: REPLICATION_STATUS_PENDING_REMOVAL.to_string(),
            role: None,
            ends: true,
        });
        Some(REPLICATION_STATUS_PENDING_REMOVAL.to_string())
    }

    /// Start the promotion of this domain to [`REPLICATION_ROLE_PRIMARY`] in its relationship
    /// with the peer `peer_domain_arn` names, and answer the status the relationship is then
    /// in — or `None` when this domain holds no such relationship.
    ///
    /// `in_flight` is the status the promotion runs under
    /// ([`REPLICATION_STATUS_SWITCHING_OVER`] for the planned promotion,
    /// [`REPLICATION_STATUS_FAILING_OVER`] for the unplanned one) and `settles_to` the status
    /// it leaves the relationship in: a switchover coordinates with the current primary and
    /// leaves replication running, while a failover EJECTS it.
    pub fn begin_promotion(
        &mut self,
        peer_domain_arn: &str,
        in_flight: &str,
        settles_to: &str,
        now: SystemTime,
    ) -> Option<String> {
        self.promote_side(
            peer_domain_arn,
            in_flight,
            settles_to,
            REPLICATION_ROLE_PRIMARY,
            now,
        )
    }

    /// The other half of [`Self::begin_promotion`], written on the PEER's own record: the
    /// domain that was the primary takes [`REPLICATION_ROLE_SECONDARY`], so a read of either
    /// side reports one relationship.
    pub fn begin_demotion(
        &mut self,
        peer_domain_arn: &str,
        in_flight: &str,
        settles_to: &str,
        now: SystemTime,
    ) -> Option<String> {
        self.promote_side(
            peer_domain_arn,
            in_flight,
            settles_to,
            REPLICATION_ROLE_SECONDARY,
            now,
        )
    }

    /// One side of a promotion: the relationship goes `in_flight` now and the clock owes it
    /// `settles_to` with this domain playing `role` one [`REPLICATION_CHANGE`] later.
    fn promote_side(
        &mut self,
        peer_domain_arn: &str,
        in_flight: &str,
        settles_to: &str,
        role: &str,
        now: SystemTime,
    ) -> Option<String> {
        let relationship = self.peers.get_mut(peer_domain_arn)?;
        relationship.status = in_flight.to_string();
        relationship.status_message = None;
        relationship.pending = Some(PendingMove {
            due: now + REPLICATION_CHANGE,
            status: settles_to.to_string(),
            role: Some(role.to_string()),
            ends: false,
        });
        Some(in_flight.to_string())
    }

    /// Materialise whatever the clock owes this arrangement at `now`: a relationship whose
    /// move has fallen due reaches the status that move named, takes the role it carried, and
    /// is GONE where the move was a removal.
    ///
    /// A domain left with no relationship plays no role, so the role goes with the last peer:
    /// that is what makes a domain whose relationship was torn down read exactly like one
    /// that never had one.
    pub fn settle(&mut self, now: SystemTime) {
        let mut released: Vec<String> = Vec::new();
        let mut role = self.role.clone();
        for (arn, relationship) in &mut self.peers {
            let Some(pending) = relationship.pending.clone() else {
                continue;
            };
            if now < pending.due {
                continue;
            }
            relationship.pending = None;
            if pending.ends {
                released.push(arn.clone());
                continue;
            }
            relationship.status = pending.status;
            if let Some(taken) = pending.role {
                relationship.peer_role = counterpart_role(&taken).to_string();
                role = Some(taken);
            }
        }
        for arn in released {
            self.peers.remove(&arn);
        }
        self.role = if self.peers.is_empty() { None } else { role };
    }

    /// The peers a read reports, in the one order the record holds them.
    #[must_use]
    pub fn to_peers(&self) -> Vec<ReplicationPeer> {
        self.peers.values().map(PeerRelationship::to_peer).collect()
    }
}
