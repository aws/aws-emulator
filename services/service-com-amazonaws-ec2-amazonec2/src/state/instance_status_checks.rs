// SPDX-License-Identifier: CC0-1.0

//! The STATUS CHECKS one instance reports: "Amazon EC2 performs status checks on running EC2
//! instances to identify hardware and software issues" (`DescribeInstanceStatus`).
//!
//! # What a check reports, and when
//!
//! EC2 reports two of them per instance — the SYSTEM status, which is the host the instance
//! runs on, and the INSTANCE status, which is the instance's own software and network stack —
//! each as a summary (`SummaryStatus`: `ok | impaired | initializing | insufficient-data |
//! not-applicable`) with the detail it was decided from (`StatusName` is `reachability`, and
//! `StatusType` is `passed | failed | initializing | insufficient-data`).
//!
//! A check is owed to the CLOCK, not to a call: an instance whose operating system has just
//! come up is still `initializing` — "it can take a few minutes for the instance to pass its
//! status checks" — and passes [`STATUS_CHECK_SPAN`] after that boot, whether or not anybody
//! polls. The instant it is measured from is the boot the guest actually had
//! ([`super::instance_boots`]), so two reads at one instant answer the same status and
//! reading a status writes nothing. An instance that is still STARTING has no boot to
//! measure from yet, so its checks are initializing too.
//!
//! An instance whose operating system is neither up nor coming up has no check to report at
//! all: "by default, only running instances are described", and the status of one that is
//! included anyway (through `IncludeAllInstances`) is `not-applicable` — with NO detail
//! beside it, because `StatusType` models no value for a check that does not apply.

use std::time::{Duration, SystemTime};

use super::{
    zone_id_for_zone_name, ComAmazonawsEc2Amazonec2State, InstanceRecord, InstanceStateName,
    TRANSITION_DELAY,
};
use crate::ops::types::{InstanceStatus, InstanceStatusDetails, InstanceStatusSummary};

/// How long after its guest came up an instance's status checks PASS.
///
/// AWS takes "a few minutes" and publishes no figure, so the span is this emulator's own —
/// the one every clock-owed move of this crate uses ([`TRANSITION_DELAY`]), which is never
/// zero, so a caller that polls straight after a launch or a reboot still sees the
/// `initializing` state the real service reports.
pub const STATUS_CHECK_SPAN: Duration = TRANSITION_DELAY;

/// `SummaryStatus`'s `ok` — the checks passed.
pub const SUMMARY_STATUS_OK: &str = "ok";

/// `SummaryStatus`'s `initializing` — the checks have not finished yet.
pub const SUMMARY_STATUS_INITIALIZING: &str = "initializing";

/// `SummaryStatus`'s `not-applicable` — there is nothing to check, which is what an instance
/// that is not running reports.
pub const SUMMARY_STATUS_NOT_APPLICABLE: &str = "not-applicable";

/// The one name `StatusName` models for a status detail.
pub const STATUS_NAME_REACHABILITY: &str = "reachability";

/// `StatusType`'s `passed`.
pub const STATUS_TYPE_PASSED: &str = "passed";

/// `StatusType`'s `initializing`.
pub const STATUS_TYPE_INITIALIZING: &str = "initializing";

/// The filters `DescribeInstanceStatus` documents, as their names.
///
/// From that operation's own `Filters` doc — the whole list, so a name it documents is never
/// refused and a name it does not is never silently ignored. The ones this emulator holds no
/// fact for (a scheduled event, an attached-volume or application status, a managing
/// principal) resolve to no value, so a request filtering on one selects nothing rather than
/// everything.
pub const SUPPORTED_INSTANCE_STATUS_FILTERS: &[&str] = &[
    "availability-zone",
    "availability-zone-id",
    "event.code",
    "event.description",
    "event.instance-event-id",
    "event.not-after",
    "event.not-before",
    "event.not-before-deadline",
    "instance-state-code",
    "instance-state-name",
    "instance-status.reachability",
    "instance-status.status",
    "operator.managed",
    "operator.principal",
    "system-status.reachability",
    "system-status.status",
    "attached-ebs-status.status",
    "application-status.status",
];

/// Whether `name` is a filter `DescribeInstanceStatus` documents.
pub fn instance_status_filter_supported(name: &str) -> bool {
    SUPPORTED_INSTANCE_STATUS_FILTERS.contains(&name)
}

/// The status checks `instance` reports as of `now`: `(system status, instance status)`.
///
/// Both halves answer the same way, because this emulator runs one host per instance: the
/// checks of an instance whose operating system is neither up nor coming up do not apply,
/// those of a guest that came up less than [`STATUS_CHECK_SPAN`] ago — or has not come up
/// yet — are still initializing, and those of a guest that came up before that have passed.
pub fn instance_status_checks(
    instance: &InstanceRecord,
    now: SystemTime,
) -> (InstanceStatusSummary, InstanceStatusSummary) {
    let summary = if !matches!(
        instance.state,
        InstanceStateName::Running | InstanceStateName::Pending
    ) {
        InstanceStatusSummary {
            // No detail: `StatusType` models no value for a check that does not apply, so a
            // detail here would have to carry a status the API does not publish.
            details: None,
            status: Some(SUMMARY_STATUS_NOT_APPLICABLE.to_string()),
        }
    } else if checks_have_passed(instance, now) {
        reachability_summary(SUMMARY_STATUS_OK, STATUS_TYPE_PASSED)
    } else {
        reachability_summary(SUMMARY_STATUS_INITIALIZING, STATUS_TYPE_INITIALIZING)
    };
    (summary.clone(), summary)
}

/// Whether the checks of `instance` have passed at `now`: its guest came up at least
/// [`STATUS_CHECK_SPAN`] ago.
///
/// An instance whose guest has not come up at all is still initializing — there is nothing to
/// have passed yet.
fn checks_have_passed(instance: &InstanceRecord, now: SystemTime) -> bool {
    super::instance_boots(instance)
        .last()
        .and_then(|boot| boot.booted_at.checked_add(STATUS_CHECK_SPAN))
        .is_some_and(|due| now >= due)
}

/// One summary of the reachability check: the summary status and the detail it was decided
/// from.
fn reachability_summary(status: &str, detail: &str) -> InstanceStatusSummary {
    InstanceStatusSummary {
        details: Some(vec![InstanceStatusDetails {
            // "The time when a status check failed" (`$ImpairedSince`): nothing here has
            // failed, so no instant is reported.
            impaired_since: None,
            name: Some(STATUS_NAME_REACHABILITY.to_string()),
            status: Some(detail.to_string()),
        }]),
        status: Some(status.to_string()),
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The `InstanceStatus` `instance` reports as of `now`.
    ///
    /// Every member is read off the stored instance: its id, the state it is in, the zone it
    /// is placed in (and that zone's id, through the one mapping this crate holds) and the
    /// checks above. A member this emulator holds no fact for is absent rather than filled
    /// in: no scheduled event is modelled, so `events` is empty; no attached-volume,
    /// application or performance status is measured, so none is claimed.
    pub fn instance_status_view(&self, instance: &InstanceRecord, now: SystemTime) -> InstanceStatus {
        let (system_status, instance_status) = instance_status_checks(instance, now);
        InstanceStatus {
            availability_zone: Some(instance.placement_az.clone()),
            availability_zone_id: zone_id_for_zone_name(&instance.placement_az),
            // An Outpost is a resource this emulator does not hold, so no instance is
            // reported on one.
            outpost_arn: None,
            operator: Some(super::unmanaged_operator()),
            events: None,
            instance_id: Some(instance.instance_id.clone()),
            instance_state: Some(instance.state.view()),
            instance_status: Some(instance_status),
            system_status: Some(system_status),
            attached_ebs_status: None,
            application_status: None,
            performance_status: None,
        }
    }
}

/// The values one instance holds for the `DescribeInstanceStatus` filter `name`, or `None`
/// when that name is no filter of this operation.
///
/// An empty list is a filter the instance holds NO value for — a scheduled event this
/// emulator does not model, a status it does not measure — so a request filtering on it
/// selects nothing, which is what a filter on a fact an instance does not have means.
pub fn instance_status_filter_values(
    status: &InstanceStatus,
    name: &str,
) -> Option<Vec<String>> {
    let summary_status = |summary: &Option<InstanceStatusSummary>| {
        summary
            .as_ref()
            .and_then(|summary| summary.status.clone())
            .into_iter()
            .collect::<Vec<String>>()
    };
    let reachability = |summary: &Option<InstanceStatusSummary>| {
        summary
            .as_ref()
            .and_then(|summary| summary.details.clone())
            .into_iter()
            .flatten()
            .filter(|detail| detail.name.as_deref() == Some(STATUS_NAME_REACHABILITY))
            .filter_map(|detail| detail.status)
            .collect::<Vec<String>>()
    };
    let held = match name {
        "availability-zone" => status.availability_zone.clone().into_iter().collect(),
        "availability-zone-id" => status.availability_zone_id.clone().into_iter().collect(),
        "instance-state-code" => status
            .instance_state
            .as_ref()
            .and_then(|state| state.code)
            .map(|code| code.to_string())
            .into_iter()
            .collect(),
        "instance-state-name" => status
            .instance_state
            .as_ref()
            .and_then(|state| state.name.clone())
            .into_iter()
            .collect(),
        "instance-status.status" => summary_status(&status.instance_status),
        "instance-status.reachability" => reachability(&status.instance_status),
        "system-status.status" => summary_status(&status.system_status),
        "system-status.reachability" => reachability(&status.system_status),
        "attached-ebs-status.status" => status
            .attached_ebs_status
            .as_ref()
            .and_then(|summary| summary.status.clone())
            .into_iter()
            .collect(),
        "application-status.status" => status
            .application_status
            .as_ref()
            .and_then(|summary| summary.status.clone())
            .into_iter()
            .collect(),
        "operator.managed" => status
            .operator
            .as_ref()
            .and_then(|operator| operator.managed)
            .map(|managed| managed.to_string())
            .into_iter()
            .collect(),
        "operator.principal" => status
            .operator
            .as_ref()
            .and_then(|operator| operator.principal.clone())
            .into_iter()
            .collect(),
        // Every scheduled-event filter: this emulator schedules no event, so an instance
        // holds no value for one.
        "event.code"
        | "event.description"
        | "event.instance-event-id"
        | "event.not-after"
        | "event.not-before"
        | "event.not-before-deadline" => Vec::new(),
        _ => return None,
    };
    Some(held)
}
