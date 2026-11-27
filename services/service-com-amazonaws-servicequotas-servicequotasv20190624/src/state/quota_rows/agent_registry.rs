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
/// divergence on any of `L-7F384C37`, `L-4BB62FF5` or `L-0CAE3C7F`, and that evidence
/// states this same rate for the record-listing row (`L-2ECDF0F2`), while the
/// `GetRegistry` row below carries a rate of its own — so the class has no one rate and a
/// further row of it is not filled in from here. It is left unpublished until its own
/// rate is stated.
const AGENT_REGISTRY_REQUEST_RATE: f64 = 10.0;

/// AWS Agent Registry's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `agent-registry` service code
/// (what `aws service-quotas list-aws-default-service-quotas --service-code
/// agent-registry` reports), and the recorded default-quota listing of that code for
/// which `QuotaCode` names which operation's rate, what that rate is, and the registry
/// plane's own published name, default and window.
///
/// Every row is published under the `L-` code AWS mints for it, in code order. The rest
/// of that listing's codes name no operation this catalog can cite, or state no default
/// value for the quota they publish, so they are left to the default-quota-set gap
/// rather than published under a guessed fact.
pub(in crate::state) fn agent_registry_quotas() -> Vec<QuotaRow> {
    vec![
        agent_registry_request_rate(
            "BatchGetDiscoverableRegistryRecord",
            AGENT_REGISTRY_REQUEST_RATE,
        )
        .code("L-0CAE3C7F"),
        // The one rate row of this class the evidence states a rate of its own for, and
        // the one beside `L-4BB62FF5` the registry publishes as NOT adjustable.
        agent_registry_request_rate("GetRegistry", 5.0)
            .code("L-160B222D")
            .fixed(),
        // The REGISTRY plane of an account: a count of resources rather than a call
        // rate, so it is metered by that plane's `AWS/Usage` resource count and carries
        // the five-minute window those counts are published over instead of the class's
        // one-second rate window. `Resource` is the series CloudWatch publishes the
        // count under, which is the RESOURCE TYPE the plane counts (the form the
        // resource planes of `appconfig` carry), not the plane's own name. Its published
        // sentence is the one the recorded listing states for this code, worded around
        // the registries an account may create rather than around a call rate.
        quota("Registries per Account", 10.0)
            .code("L-197AC8CB")
            .describe(
                "The maximum number of registries that can be created per account per Region",
            )
            .metric(usage_metric(
                AGENT_REGISTRY_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "Registry"),
                    ("Type", "Resource"),
                ],
            )),
        agent_registry_request_rate("ListRegistryRecords", AGENT_REGISTRY_REQUEST_RATE)
            .code("L-2ECDF0F2")
            // One of the rate rows this registry publishes as NOT adjustable, as the
            // recorded listing states for this code.
            .fixed(),
        agent_registry_request_rate("DeleteRegistryRecord", AGENT_REGISTRY_REQUEST_RATE)
            .code("L-4BB62FF5")
            // The one row of this class the recorded listing publishes as NOT
            // adjustable, against the registry's other rate rows.
            .fixed(),
        agent_registry_request_rate("InvokeRegistryMcp", AGENT_REGISTRY_REQUEST_RATE)
            .code("L-7F384C37"),
    ]
}
