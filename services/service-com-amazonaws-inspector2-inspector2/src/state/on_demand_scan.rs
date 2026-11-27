// SPDX-License-Identifier: CC0-1.0

//! The ON-DEMAND SCANS of a resource — the scans `StartOnDemandScan` records,
//! `GetOnDemandScanStatus` reports one of and `ListOnDemandScans` pages through.
//!
//! A scan is a RECORD of work the service started at an instant, so everything a read
//! reports about one is derived from that record and the clock: a scan is still running
//! until the span the service takes to finish it has fallen due
//! ([`ON_DEMAND_SCAN_DURATION`]), and from that instant it is finished and reports the
//! instant it finished at. Nothing is written when a scan is read, so two reads of one
//! scan agree and a caller polling a scan sees exactly what any other caller does.
//!
//! BOUNDED by [`MAX_ON_DEMAND_SCANS`]: `StartOnDemandScan` is the only operation that adds
//! a record and refuses once the store holds that many, so a caller looping it cannot grow
//! the store without limit.

use std::time::{Duration, SystemTime};

use super::{timestamp, ComAmazonawsInspector2Inspector2State};

/// The `OnDemandScanStatus` token of a scan the service is still running.
/// Source: `smithy_query(query="shape", shape="OnDemandScanStatus")` —
/// `IN_PROGRESS | SUCCESSFUL | FAILED`.
pub const ON_DEMAND_SCAN_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `OnDemandScanStatus` token of a scan that has finished its work.
/// Source: `smithy_query(query="shape", shape="OnDemandScanStatus")`.
pub const ON_DEMAND_SCAN_SUCCESSFUL: &str = "SUCCESSFUL";

/// How long an on-demand scan runs before the clock owes it its terminal status. The real
/// service scans asynchronously and no AWS document fixes how long one takes, so this span
/// is this emulator's own: non-zero, so a caller reading straight after the start still
/// observes a scan in progress, and short enough that polling settles quickly.
pub const ON_DEMAND_SCAN_DURATION: Duration = Duration::from_secs(3);

/// How many on-demand scans this emulator keeps records for. `StartOnDemandScan` refuses
/// once the store holds this many, so the one map a request-supplied resource id and client
/// token reach cannot grow without limit.
pub const MAX_ON_DEMAND_SCANS: usize = 1_000;

/// The longest a `ScanId` may be.
/// Source: `smithy_query(query="shape", shape="ScanId")` — `@length` 1..64, `@pattern`
/// `^[a-z0-9]+$`.
const MAX_SCAN_ID_LENGTH: usize = 64;

/// How this service words its refusal of a request whose `clientToken` it already minted a
/// scan of ANOTHER resource for: an idempotency token names one request, so the same token
/// cannot name two resources. The conflicting resource is named by the exception's own
/// `resourceId` and `resourceType` members, so the sentence itself carries no value from
/// the request.
pub const CLIENT_TOKEN_CONFLICT_MESSAGE: &str =
    "The clientToken of this request was already used to start an on-demand scan of a \
     different resource.";

/// What this service states when it holds as many on-demand scans as it keeps records for.
pub fn too_many_on_demand_scans_message() -> String {
    format!(
        "This emulator keeps records for at most {MAX_ON_DEMAND_SCANS} on-demand scans, and \
         holds that many."
    )
}

/// Whether `value` is shaped like a `ScanId`, which is what makes a decoded page cursor a
/// position in the scan list rather than an arbitrary string.
/// Source: `smithy_query(query="shape", shape="ScanId")` — `@length` 1..64, `@pattern`
/// `^[a-z0-9]+$`.
pub fn is_scan_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCAN_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// One recorded on-demand scan of one resource.
#[derive(Debug, Clone)]
pub struct StoredOnDemandScan {
    /// The scan's own id, which is also its key in the store.
    pub scan_id: String,
    /// The `resourceId` the scan was started for, exactly as the request sent it.
    pub resource_id: String,
    /// The `OnDemandResourceType` token the scan was started for.
    pub resource_type: String,
    /// The instant the scan was started, which is what `startedAt` reports.
    pub started_at: SystemTime,
    /// The instant the scan's work falls due — the transition the CLOCK owes, stored
    /// rather than slept for, so both reads derive the status and report nothing they
    /// wrote.
    pub completes_at: SystemTime,
    /// The `clientToken` the start carried, absent when it carried none. A retry carrying
    /// the same token is a REPLAY of this scan rather than a second scan.
    pub client_token: Option<String>,
}

impl StoredOnDemandScan {
    /// The `OnDemandScanStatus` this scan reports at `now`: still running until its work
    /// falls due, and finished from that instant on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.completes_at {
            ON_DEMAND_SCAN_IN_PROGRESS
        } else {
            ON_DEMAND_SCAN_SUCCESSFUL
        }
    }

    /// The instant this scan completed, and `None` while it is still running — which is
    /// what `completedAt` reports, absent for a scan that has not completed.
    pub fn completed_at(&self, now: SystemTime) -> Option<SystemTime> {
        (now >= self.completes_at).then_some(self.completes_at)
    }

    /// Whether this scan is the one a request naming `resource_id` and `resource_type`
    /// asks about: a scan belongs to the resource it was started for, so a scan id paired
    /// with another resource names no scan.
    pub fn is_of(&self, resource_id: &str, resource_type: &str) -> bool {
        self.resource_id == resource_id && self.resource_type == resource_type
    }
}

/// One recorded scan as the wire `OnDemandScan`, at the instant it was read.
///
/// `scanStatusReason` is left ABSENT: every token it admits is a reason a scan FAILED
/// (`smithy_query(query="shape", shape="OnDemandScanStatusReason")`), and a scan this
/// emulator started runs to completion, so there is no reason to report.
pub fn on_demand_scan_view(
    stored: &StoredOnDemandScan,
    now: SystemTime,
) -> crate::ops::types::OnDemandScan {
    crate::ops::types::OnDemandScan {
        scan_id: stored.scan_id.clone(),
        resource_id: stored.resource_id.clone(),
        resource_type: stored.resource_type.clone(),
        scan_status: stored.status(now).to_string(),
        scan_status_reason: None,
        started_at: timestamp(stored.started_at),
        completed_at: stored.completed_at(now).map(timestamp),
    }
}

/// What a `StartOnDemandScan` call did: the scan it is answered with, and whether this call
/// is what recorded it.
#[derive(Debug)]
pub enum StartedOnDemandScan {
    /// The scan this call recorded.
    Started(String),
    /// The scan a previous call with the same `clientToken` recorded, which this call
    /// replays rather than scanning the resource twice.
    Replayed(String),
    /// The call's `clientToken` already names a scan of ANOTHER resource, named here as the
    /// conflicting `(resourceId, resourceType)` pair.
    TokenConflict(String, String),
    /// The store already holds as many scans as this emulator keeps records for, so
    /// nothing was recorded.
    Full,
}

impl ComAmazonawsInspector2Inspector2State {
    /// The scan `scan_id` names, whatever resource it was started for.
    pub fn on_demand_scan(&self, scan_id: &str) -> Option<&StoredOnDemandScan> {
        self.on_demand_scans.get(scan_id)
    }

    /// Every recorded scan of one resource, in the order they were started — which is the
    /// order their ids sort in, so a page cursor names a position that stays correct when
    /// scans are added between pages.
    pub fn on_demand_scans_of<'a>(
        &'a self,
        resource_id: &'a str,
        resource_type: &'a str,
    ) -> impl Iterator<Item = &'a StoredOnDemandScan> {
        self.on_demand_scans
            .values()
            .filter(move |stored| stored.is_of(resource_id, resource_type))
    }

    /// Record a scan of `resource_id` started at `now`, or report why none was recorded.
    ///
    /// `client_token` is the request's `@idempotencyToken`: a call carrying a token a scan
    /// was already started with is a REPLAY, answered with that scan's id, so a retried
    /// request scans the resource once. The same token naming a DIFFERENT resource is a
    /// conflict instead — one token cannot name two scans — and nothing is recorded for it.
    ///
    /// Adds at most one entry, and only below [`MAX_ON_DEMAND_SCANS`], so the caller
    /// refuses the request rather than reporting a scan this store does not hold.
    #[must_use]
    pub fn start_on_demand_scan(
        &mut self,
        resource_id: &str,
        resource_type: &str,
        client_token: Option<&str>,
        now: SystemTime,
    ) -> StartedOnDemandScan {
        if let Some(token) = client_token {
            if let Some(recorded) = self
                .on_demand_scans
                .values()
                .find(|stored| stored.client_token.as_deref() == Some(token))
            {
                if recorded.is_of(resource_id, resource_type) {
                    return StartedOnDemandScan::Replayed(recorded.scan_id.clone());
                }
                return StartedOnDemandScan::TokenConflict(
                    recorded.resource_id.clone(),
                    recorded.resource_type.clone(),
                );
            }
        }

        if self.on_demand_scans.len() >= MAX_ON_DEMAND_SCANS {
            return StartedOnDemandScan::Full;
        }

        self.next_on_demand_scan_id += 1;
        // A zero-padded 16-hex-digit counter: inside `ScanId`'s modelled 1..64 length and
        // its `^[a-z0-9]+$` pattern, and ordered the way the scans were started, so no two
        // scans of one emulator run collide and the list's order is the store's.
        let scan_id = format!("{:016x}", self.next_on_demand_scan_id);
        self.on_demand_scans.insert(
            scan_id.clone(),
            StoredOnDemandScan {
                scan_id: scan_id.clone(),
                resource_id: resource_id.to_string(),
                resource_type: resource_type.to_string(),
                started_at: now,
                completes_at: now + ON_DEMAND_SCAN_DURATION,
                client_token: client_token.map(str::to_string),
            },
        );
        StartedOnDemandScan::Started(scan_id)
    }
}
