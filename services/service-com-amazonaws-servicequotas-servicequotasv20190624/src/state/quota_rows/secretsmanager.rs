// SPDX-License-Identifier: CC0-1.0

//! AWS Secrets Manager's published default-quota catalog.

use super::*;

/// The name `AWS/Usage` publishes AWS Secrets Manager's call counts under, which is the
/// `Service` dimension of every metered row of that catalog.
const SECRETS_MANAGER_USAGE_SERVICE: &str = "Secrets Manager";

/// The `Description` AWS Secrets Manager publishes for one of its rate quotas: the
/// TRANSACTIONS PER SECOND it serves the named API requests at.
///
/// SOURCE: the recorded default-quota listing of `secretsmanager`, which carries this
/// sentence for every rate it publishes. `apis` names the operation the rate throttles,
/// and `combined` is what the service appends where ONE rate is published over several
/// operations at once.
fn secrets_manager_rate_description(apis: &str, combined: bool) -> String {
    let combined = if combined { " combined" } else { "" };
    format!("The maximum transactions per second for {apis} API requests{combined}.")
}

/// One AWS Secrets Manager API REQUEST-RATE quota: `Rate of <api> API requests`, the
/// most requests to that one operation Secrets Manager serves per second.
///
/// Secrets Manager publishes a rate quota for each of its API operations, and every
/// member of such a row but the operation's own name and the value is the same across
/// the class: the `QuotaName` names the operation whose requests the rate throttles,
/// the window is one second, the usage metric is that operation's `AWS/Usage`
/// `CallCount` ([`call_count_metric`]) under the service's `AWS/Usage` display name,
/// and the `Description` is the sentence this service publishes for a rate quota
/// ([`secrets_manager_rate_description`]).
fn secrets_manager_request_rate(api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(
        format!("Rate of {api_name} API requests"),
        requests_per_second,
    )
    .period(1, "SECOND")
    .metric(call_count_metric(SECRETS_MANAGER_USAGE_SERVICE, api_name))
    .describe(&secrets_manager_rate_description(api_name, false))
}

/// One rate Secrets Manager publishes over SEVERAL operations at once: `Combined rate of
/// <apis> API requests`.
///
/// No single operation's call count meters such a rate, so the row carries the window and
/// the sentence of the class ([`secrets_manager_rate_description`]) and no usage metric.
/// `apis` is the operation list as the service publishes it, serial comma included.
fn secrets_manager_combined_rate(apis: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("Combined rate of {apis} API requests"), requests_per_second)
        .period(1, "SECOND")
        .describe(&secrets_manager_rate_description(apis, true))
}

/// AWS Secrets Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `secretsmanager` service code
/// (the "Quotas for AWS Secrets Manager" page of the AWS Secrets Manager User Guide),
/// and the recorded default-quota listing of that code for which `QuotaCode` names
/// which quota, for the sentence published under each and for the adjustability of
/// each: this service publishes its request rates, and the secret and secret-value
/// planes beside them, as NOT adjustable, which is why every row below is fixed. Two of
/// the rates are published as a COMBINED rate over several operations at once, so no
/// single operation's call count meters them. The rest of the
/// service's published catalog names no quota this catalog can cite under the remaining
/// codes that listing reports, so those rows are left to the default-quota-set gap
/// rather than published under a guessed binding (see
/// [`super::descriptive_quota_code`]).
pub(in crate::state) fn secretsmanager_quotas() -> Vec<QuotaRow> {
    vec![
        secrets_manager_request_rate("GetSecretValue", 10_000.0)
            .code("L-2D877201")
            .fixed(),
        // The secret-VALUE plane: a size in bytes rather than a rate, which the recorded
        // listing publishes under this code as NOT adjustable, so no increase can be
        // asked for it. Its sentence is not among the data available here, so it is
        // written in the voice this service words a measured plane with rather than
        // cited, as is the secret plane's below.
        quota("Secret value size", 65_536.0)
            .code("L-2F24C883")
            .unit("Bytes")
            .fixed()
            .describe("The maximum size of a secret value."),
        // The SECRET plane of the account, which the recorded listing publishes under
        // this code as not adjustable at its published default.
        quota("Secrets", 500_000.0)
            .code("L-2F66C23C")
            .fixed()
            .describe(
                "The maximum number of secrets in this account in the current Region.",
            ),
        secrets_manager_request_rate("DescribeSecret", 40_000.0)
            .code("L-385CE7B7")
            .fixed(),
        secrets_manager_request_rate("GetRandomPassword", 50.0)
            .code("L-6A9CA93A")
            .fixed(),
        secrets_manager_combined_rate(
            "DeleteResourcePolicy, GetResourcePolicy, PutResourcePolicy, and \
             ValidateResourcePolicy",
            50.0,
        )
        .code("L-8D82A813")
        .fixed(),
        // The tagging plane's combined rate. Its name, sentence, window and
        // adjustability are the recorded listing's; the rate is the one the service
        // publishes for its combined planes, which is the one member of the row no
        // source available here states.
        secrets_manager_combined_rate("TagResource and UntagResource", 50.0)
            .code("L-BC8DD7C4")
            .fixed(),
        secrets_manager_request_rate("CreateSecret", 50.0)
            .code("L-CBA4CAC4")
            .fixed(),
        secrets_manager_request_rate("BatchGetSecretValue", 100.0)
            .code("L-F452D4B5")
            .fixed(),
    ]
}
