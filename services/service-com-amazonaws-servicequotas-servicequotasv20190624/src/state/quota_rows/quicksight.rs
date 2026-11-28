// SPDX-License-Identifier: CC0-1.0

//! Amazon QuickSight's published default-quota catalog.

use super::*;

/// Amazon QuickSight's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `quicksight` service code, and the
/// recorded default-quota listing of that code for which `QuotaCode` names which quota,
/// for the value, unit and adjustability each publishes and for the `Description` the
/// service publishes under a code the listing states one for: the service publishes the
/// planes of a space and of the account beside the retention, size and nesting limits of
/// its agent features, which is why the rows are named for what the quota bounds rather
/// than for a resource type. The remaining codes that listing reports name quotas whose
/// published name or value no source available here states, and a code identifies ONE
/// quota, so those rows are left to the default-quota-set gap rather than published under
/// a guessed binding (see [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn quicksight_quotas() -> Vec<QuotaRow> {
    vec![
        // A depth AWS publishes as a hard limit of the automation.
        quota("Amazon Quick Automate Maximum Nesting Depth", 6.0)
            .code("L-12A1F6CA")
            .fixed()
            .describe("Maximum nesting depth allowed in an automation in Amazon Quick Automate"),
        // A size rather than a count, so the row carries the unit AWS measures it in,
        // and the sentence AWS publishes for it names that unit.
        quota(
            "Maximum total size of attached reference documents per chat agent",
            50.0,
        )
        .code("L-26532294")
        .unit("Megabytes")
        .fixed()
        .describe(
            "The maximum size in megabytes (MBs) of all reference documents attached to a chat \
             agent.",
        ),
        // The resource plane of a space, which AWS publishes as fixed.
        quota("Maximum number of resources per space", 100.0)
            .code("L-39A09B2E")
            .fixed()
            .describe("The maximum number of resources that can be linked to a space."),
        // The hours an approved session stays approved for, which AWS publishes as a hard
        // limit, and the automations of an account, which it publishes as adjustable. The
        // sentence AWS publishes for each is not among the data available here, so neither
        // row carries one.
        quota("Session approval expiry in hours", 12.0)
            .code("L-3B893AF4")
            .fixed(),
        quota("Amazon Quick Automate Maximum Automations", 1000.0).code("L-5FE00BED"),
        quota("Scheduled task history retention in days", 365.0)
            .code("L-7DB5CFE0")
            .fixed()
            .describe("The number of days scheduled task run history is retained."),
        // A size measured in gigabytes rather than a count.
        quota("Maximum custom agent files size", 1.0)
            .code("L-8926DD84")
            .unit("Gigabytes"),
        quota("Custom agent artifact retention in days", 365.0)
            .code("L-90F283DF")
            .fixed()
            .describe("The number of days custom agent artifacts are retained before deletion."),
        quota("Maximum number of approval policies per account", 500.0)
            .code("L-D75C2D48")
            .describe(
                "The maximum number of approval policies that can be created in a single account \
                 across all asset types.",
            ),
    ]
}
