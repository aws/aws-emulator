// SPDX-License-Identifier: CC0-1.0

//! AWS Agent Registry's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name CloudWatch publishes AWS Agent Registry's call counts
/// under, which is the `Service` dimension of every metered row of this catalog.
const AGENT_REGISTRY_USAGE_SERVICE: &str = "Agent Registry";

/// One AWS Agent Registry API REQUEST-RATE quota: `<api> requests per second`, the most
/// requests to that one operation the registry serves per second per account in a
/// Region.
///
/// The registry publishes a rate quota for each of its API operations, and every member
/// of such a row but the operation's own name and the limit is the same across the
/// class: the `QuotaName` is the operation's name followed by `requests per second`, the
/// value is counted in `Count/Second`, the window is one second, the usage metric is
/// that operation's `AWS/Usage` `CallCount` ([`call_count_metric`]), and the
/// `Description` is the sentence the class is published with, which names the account
/// and the Region the rate is counted on. Those are the quota's PUBLISHED members, so
/// one constructor fills them and two rows of the class cannot drift apart.
///
/// SOURCE: the recorded default-quota listing of `agent-registry`, which this catalog's
/// `L-7F384C37` row is cited from — including that row's published sentence, stated
/// verbatim by the evidence behind this catalog — for the class's name form, its unit,
/// its one-second window, its call-count metric and the sentence above.
///
/// The per-second LIMIT and `Adjustable` are NOT facts of the class: each belongs to the
/// one operation the row is published for, so both are stated per row
/// ([`AGENT_REGISTRY_REQUEST_RATE`], [`agent_registry_quotas`]).
fn agent_registry_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("{api_name} requests per second"),
        requests_per_second,
    )
    .unit("Count/Second")
    .period(1, "SECOND")
    .metric(call_count_metric(AGENT_REGISTRY_USAGE_SERVICE, api_name))
    .describe(&format!(
        "The maximum number of {api_name} API requests per second per account per Region"
    ))
}

/// The requests per second each of the three rows below is published at.
///
/// SOURCE: the recorded default-quota listing of `agent-registry`. It is cited PER ROW,
/// not as a fact of the class: a replay of that listing against this catalog reports no
/// divergence on any of `L-7F384C37`, `L-4BB62FF5` or `L-0CAE3C7F`, so the rate is the
/// one that listing states for each of them. A further row of the class is not filled in
/// from here — it is left unpublished until its own rate is stated.
const AGENT_REGISTRY_REQUEST_RATE: f64 = 10.0;

/// AWS Agent Registry's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `agent-registry` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// agent-registry` reports), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which operation's rate.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The
/// registry COUNT quota that listing carries (`L-197AC8CB`, the registries of an
/// account, published with a five-minute resource-count window) states no default value
/// this catalog can source, and the rest of its codes name no operation this catalog can
/// cite, so both are left to the default-quota-set gap rather than published under a
/// guessed fact.
pub(in crate::state) fn agent_registry_quotas() -> Vec<QuotaRow> {
    vec![
        agent_registry_request_rate(
            "BatchGetDiscoverableRegistryRecord",
            AGENT_REGISTRY_REQUEST_RATE,
        )
        .code("L-0CAE3C7F"),
        agent_registry_request_rate("DeleteRegistryRecord", AGENT_REGISTRY_REQUEST_RATE)
            .code("L-4BB62FF5")
            // The one row of this class the recorded listing publishes as NOT
            // adjustable, against the registry's other rate rows.
            .fixed(),
        agent_registry_request_rate("InvokeRegistryMcp", AGENT_REGISTRY_REQUEST_RATE)
            .code("L-7F384C37"),
    ]
}
