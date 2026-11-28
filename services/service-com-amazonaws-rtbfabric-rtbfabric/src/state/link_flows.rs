// SPDX-License-Identifier: CC0-1.0

//! The one application of a flow-configuration change to a link.
//!
//! `PutLinkFlows` addresses a link through the gateway it is on, which is how
//! every link this store holds is addressed: a link between two gateways
//! ([`Link`](super::Link)) and a link with one end outside the fabric, of either
//! direction. The three are separate records — only a link between two gateways
//! has a peer, which is why — but a change to the flows one of them runs follows
//! the same rules whichever kind it is: the precondition the caller presents has
//! to be the link's current one, a link that has stopped where nothing moves it
//! from has no flows left to configure, the whole configuration is replaced, and
//! the link serves its prior flows until the change commits. So the application
//! lives here once, and the handler states the rules rather than repeating them
//! three times.

use std::time::SystemTime;

use crate::ops::types::Flow;

use super::{
    link_status_conflict_message, link_status_is_up, link_status_terminal, ComAmazonawsRtbfabricRtbfabricState,
    Flows, LINK_STATUS_PENDING_UPDATE, PROVISIONING_DELAY,
};

/// What an accepted flow-configuration change leaves to report.
#[derive(Debug, Clone)]
pub struct FlowsAccepted {
    /// The link the change was accepted on.
    pub link_id: String,
    /// The status the link now stands in: `PENDING_UPDATE` for a link that was
    /// carrying traffic, since the change applies while it keeps serving its
    /// prior flows, and otherwise the status the link's own lifecycle owes it.
    pub status: String,
}

/// Why a flow-configuration change was not accepted.
#[derive(Debug, Clone)]
pub enum FlowsRefused {
    /// The addressed gateway holds no link of that id, of any kind.
    NotFound,
    /// The link's current status admits no change: carries the words the refusal
    /// states.
    Status(String),
    /// The precondition presented is not the link's current one: carries the
    /// words the refusal states.
    StaleLockToken(String),
}

/// What the words of a stale-precondition refusal are: the link and the gateway
/// the call addressed, and what the caller does about it. The token itself is
/// never echoed — a caller that presented a stale one learns the current one by
/// reading the link, not from the refusal.
fn stale_lock_token_message(link_id: &str, gateway_id: &str) -> String {
    format!(
        "the lockToken presented for link {link_id} on gateway {gateway_id} is not the link's \
         current one: read the link again and retry with the lockToken it reports"
    )
}

/// One link's members a flow-configuration change reads and writes, borrowed out
/// of whichever record holds them.
///
/// The three link records hold these five members under different names around
/// them, so this is what lets the one application below run on any of them.
struct Addressed<'record> {
    link_id: &'record str,
    status: &'record mut String,
    settles_at: &'record mut Option<SystemTime>,
    updated_at: &'record mut SystemTime,
    flows: &'record mut Flows,
    revision: &'record mut u64,
}

impl Addressed<'_> {
    /// Apply the change, as every kind of link takes it.
    fn accept(
        self,
        gateway_id: &str,
        presented_lock_token: &str,
        flows: Vec<Flow>,
        now: SystemTime,
    ) -> Result<FlowsAccepted, FlowsRefused> {
        if link_status_terminal(self.status) {
            // Nothing moves the link out of this status, so it has no flows left
            // to configure.
            return Err(FlowsRefused::Status(link_status_conflict_message(
                self.link_id,
                gateway_id,
                self.status,
                "configured",
            )));
        }
        // The precondition: the token the caller read has to be the one the link
        // stands at now. It is checked AFTER the retry of an accepted change is
        // answered — a retry reports the update it started rather than being
        // measured against a precondition its own change made stale — and BEFORE
        // anything is written, so a refused call changes nothing.
        if presented_lock_token != super::lock_token(self.link_id, *self.revision) {
            return Err(FlowsRefused::StaleLockToken(stale_lock_token_message(
                self.link_id,
                gateway_id,
            )));
        }

        // `flows` is required and states the WHOLE configuration, so what it
        // carries replaces whatever the link had taken — the empty list included,
        // which is what removes every flow. It is held as the configuration
        // awaiting the link and put in service when the link is, so a reader sees
        // the flows actually running until the change commits.
        self.flows.accept(flows);
        *self.updated_at = now;
        // A change to the link has been accepted, so the precondition a caller
        // read before this call goes stale.
        *self.revision = self.revision.saturating_add(1);
        if link_status_is_up(self.status) {
            // The link is carrying traffic, so the configuration is being changed
            // under live traffic: the call reports the update as pending, and the
            // clock owes the link its way back up with the change in service.
            *self.status = LINK_STATUS_PENDING_UPDATE.to_string();
            *self.settles_at = Some(now + PROVISIONING_DELAY);
        }
        // A link that is not up yet keeps the status its own lifecycle owes it;
        // the configuration it has just taken comes into service with the link.

        Ok(FlowsAccepted {
            link_id: self.link_id.to_string(),
            status: self.status.clone(),
        })
    }
}

impl ComAmazonawsRtbfabricRtbfabricState {
    /// Accept the flow configuration `flows` on the link `link_id` addressed
    /// through the gateway `gateway_id`, whichever kind of link that is.
    ///
    /// `presented_lock_token` is the concurrency precondition the caller read
    /// from that link. Nothing is written unless the change is accepted.
    pub fn accept_link_flows(
        &mut self,
        gateway_id: &str,
        link_id: &str,
        presented_lock_token: &str,
        flows: Vec<Flow>,
        now: SystemTime,
    ) -> Result<FlowsAccepted, FlowsRefused> {
        let addressed = if let Some(link) = self.find_link_mut(gateway_id, link_id) {
            Addressed {
                link_id: &link.link_id,
                status: &mut link.status,
                settles_at: &mut link.settles_at,
                updated_at: &mut link.updated_at,
                flows: &mut link.flows,
                revision: &mut link.revision,
            }
        } else if let Some(link) = self.find_inbound_external_link_mut(gateway_id, link_id) {
            Addressed {
                link_id: &link.link_id,
                status: &mut link.status,
                settles_at: &mut link.settles_at,
                updated_at: &mut link.updated_at,
                flows: &mut link.flows,
                revision: &mut link.revision,
            }
        } else if let Some(link) = self.find_outbound_external_link_mut(gateway_id, link_id) {
            Addressed {
                link_id: &link.link_id,
                status: &mut link.status,
                settles_at: &mut link.settles_at,
                updated_at: &mut link.updated_at,
                flows: &mut link.flows,
                revision: &mut link.revision,
            }
        } else {
            return Err(FlowsRefused::NotFound);
        };
        addressed.accept(gateway_id, presented_lock_token, flows, now)
    }

    /// The concurrency precondition the link `link_id` on the gateway
    /// `gateway_id` stands at, whichever kind of link it is; `None` when that
    /// gateway holds no link of that id.
    ///
    /// What every read of a link hands out, and what a conditional write presents
    /// back.
    pub fn link_lock_token(&self, gateway_id: &str, link_id: &str) -> Option<String> {
        if let Some(link) = self.find_link(gateway_id, link_id) {
            Some(link.lock_token())
        } else if let Some(link) = self.find_inbound_external_link(gateway_id, link_id) {
            Some(link.lock_token())
        } else {
            self.find_outbound_external_link(gateway_id, link_id)
                .map(|link| link.lock_token())
        }
    }

    /// The status the link `link_id` on the gateway `gateway_id` stands in,
    /// whichever kind of link it is; `None` when that gateway holds no link of
    /// that id.
    pub fn link_status(&self, gateway_id: &str, link_id: &str) -> Option<String> {
        if let Some(link) = self.find_link(gateway_id, link_id) {
            Some(link.status.clone())
        } else if let Some(link) = self.find_inbound_external_link(gateway_id, link_id) {
            Some(link.status.clone())
        } else {
            self.find_outbound_external_link(gateway_id, link_id)
                .map(|link| link.status.clone())
        }
    }
}
