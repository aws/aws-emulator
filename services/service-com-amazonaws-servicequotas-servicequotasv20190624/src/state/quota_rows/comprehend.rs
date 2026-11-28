// SPDX-License-Identifier: CC0-1.0

//! Amazon Comprehend's published default-quota catalog.

use super::*;

/// One Amazon Comprehend API THROTTLE quota: the transactions per second Comprehend
/// serves calls of that one operation at.
///
/// The service publishes these as `<Api> throttle limit in transactions per second`, so
/// the window is stated in the quota's own name and the rows carry no `Period` or
/// `UsageMetric` block. Every member of such a row but the operation's own name, the
/// value and whether an increase can be requested is the same across the class — the
/// `Description` is the sentence the service publishes for the class with the operation
/// named in it — so one constructor fills them and two rows of the class cannot drift
/// apart.
///
/// SOURCE: AWS's published Service Quotas data for the `comprehend` service code (the
/// "Amazon Comprehend endpoints and quotas" data of its Developer Guide), and the
/// recorded default-quota listing of that code for the class's wording and for which
/// `QuotaCode` names which operation's throttle.
fn comprehend_throttle(code: &str, api_name: &str, transactions_per_second: f64) -> QuotaRow {
    quota(
        format!("{api_name} throttle limit in transactions per second"),
        transactions_per_second,
    )
    .code(code)
    .describe(&format!(
        "The maximum number of {api_name} requests allowed per account per second, in the \
         current Region"
    ))
}

/// One Amazon Comprehend ACTIVE-JOB quota: the number of jobs that one asynchronous
/// operation may have running at once.
///
/// A second class of this service's catalog, published as `<Api> max active jobs` with
/// the sentence the service publishes for the class — a count of jobs rather than a rate,
/// so it carries no window.
///
/// SOURCE: the same published data and recorded listing as [`comprehend_throttle`], for
/// the class's wording and for which `QuotaCode` names which operation's job count.
fn comprehend_active_jobs(code: &str, api_name: &str, jobs: f64) -> QuotaRow {
    quota(format!("{api_name} max active jobs"), jobs)
        .code(code)
        .describe(&format!(
            "The maximum number of active jobs for API {api_name} allowed per account, in the \
             current Region"
        ))
}

/// Amazon Comprehend's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `comprehend` service code (the
/// "Amazon Comprehend endpoints and quotas" data of its Developer Guide), and the
/// recorded default-quota listing of that code for which `QuotaCode` names which quota,
/// for the value each publishes and for whether an increase can be requested for it: the
/// service publishes a per-operation throttle ([`comprehend_throttle`]) beside the number
/// of jobs one operation may have running ([`comprehend_active_jobs`]) and the number of
/// RESOURCES of a kind an account may have active at once. The rest of that
/// listing's codes name operations no source available here states, and a code identifies
/// ONE quota, so those rows are left to the default-quota-set gap rather than published
/// under a guessed binding (see [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn comprehend_quotas() -> Vec<QuotaRow> {
    vec![
        comprehend_throttle("L-1F3990DA", "DetectDominantLanguage", 50.0),
        comprehend_active_jobs("L-2B8ECCAB", "StartEntitiesDetectionJob", 10.0),
        // The two ACTIVE-RESOURCE counts the recorded listing of this service code states
        // the published name, value and adjustability of: the endpoints an account may
        // have active, and the flywheels it may have active. Neither is a throttle, so
        // neither carries the throttle class's name or sentence — and the sentence AWS
        // publishes for each is not one any source available here states, so the row
        // carries none rather than wording authored here.
        quota("Endpoints max active endpoints", 20.0).code("L-55642075"),
        // The throttles of this service the recording states as NOT adjustable; the rest
        // of the class is published as adjustable, which is what the class constructor
        // builds.
        comprehend_throttle("L-9A845B3A", "TagResource", 1.0).fixed(),
        quota("CreateFlywheel max active flywheels", 20.0).code("L-AE5B911F"),
        comprehend_throttle("L-C7E12CA2", "StopKeyPhrasesDetectionJob", 1.0),
        comprehend_throttle("L-CF92EEE2", "StopTrainingEntityRecognizer", 1.0).fixed(),
        comprehend_throttle("L-E7F70588", "DeleteEntityRecognizer", 1.0).fixed(),
        comprehend_throttle("L-F039F1E3", "DescribeEntityRecognizer", 10.0).fixed(),
    ]
}
