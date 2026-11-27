// SPDX-License-Identifier: CC0-1.0

//! AWS DevOps Agent's published default-quota catalog.

use super::*;

/// AWS DevOps Agent's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `aidevops` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code aidevops`
/// reports), and the recorded default-quota listing of that code for which `QuotaCode`
/// names which plane, its published name, unit, default and adjustability.
///
/// THE SHAPE OF THIS SERVICE'S ROWS: it names a quota with the hyphenated SLUG of what
/// the quota counts. The sentence it publishes for a row is the row's OWN published text:
/// for the agent-space plane (`L-510AF4A9`) and the evaluation plane (`L-4FF187E6`) that
/// is each row's slug read as prose, while the memory, memory-store and on-demand
/// invocation planes carry the sentences the recorded default-quota listing of this code
/// states for them — which is why the slug reading is not generated for every row.
///
/// Every row is published under the `L-` code AWS mints for it, in code order; the
/// INVESTIGATION plane an agent space may run at once (`L-1553F789`) is published under
/// its code at the default, unit and adjustability that evidence states for it.
pub(in crate::state) fn aidevops_quotas() -> Vec<QuotaRow> {
    vec![
        // The investigations one agent space may run at once, published under its own
        // code with the unit, default and adjustability the recorded listing states. Its
        // own published sentence is not among the data available here — that listing
        // shows the evaluation plane's wording around this plane's slug is not what this
        // row carries, and withholds the text — so the row publishes none rather than
        // wording of this catalog's own.
        quota("concurrent-investigations-per-agent-space", 3.0)
            .code("L-1553F789")
            .unit("Count"),
        quota("concurrent-on-demand-invocations-per-agent-space", 10.0)
            .code("L-3D8A06B8")
            .unit("Count")
            .describe("Concurrent on-demand agent invocations per agent space"),
        quota("concurrent-evaluations-per-agent-space", 1.0)
            .code("L-4FF187E6")
            .unit("Count")
            .fixed()
            .describe("Concurrent evaluations per agent space"),
        // The agent-space plane of the account, the one row of this service published
        // under the countable `None` rather than under `Count`.
        quota("agent-spaces-per-account-per-region", 100.0)
            .code("L-510AF4A9")
            .describe("Agent spaces per account per Region"),
        // The memory planes, which AWS publishes as NOT adjustable — against the
        // on-demand-invocation plane above, whose increase it takes — each with the
        // sentence the recorded listing states for its own code.
        quota("memory-stores-per-agent-space", 50.0)
            .code("L-D774F842")
            .unit("Count")
            .fixed()
            .describe(
                "The maximum number of memory stores that can exist in a single agent \
                 space.",
            ),
        quota("memories-per-memory-store", 200.0)
            .code("L-F1DF5F94")
            .unit("Count")
            .fixed()
            .describe(
                "The maximum number of individual memory files within a single memory \
                 store.",
            ),
    ]
}
