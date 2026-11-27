// SPDX-License-Identifier: CC0-1.0

//! The deployment-configuration half of this service's state: the JOBS
//! `StartDeploymentConfigurationJob` records, and the ACTIVE configuration a job
//! that succeeded published.
//!
//! A deployment configuration "defines how buyers access a product's delivery
//! option after they subscribe", and it is published by a JOB rather than
//! directly: `StartDeploymentConfigurationJob` "starts a job that publishes a
//! deployment configuration for a product and delivery option. The job runs
//! asynchronously. Use GetDeploymentConfigurationJob to track its status. When the
//! job reaches the SUCCEEDED status, its configuration becomes the active
//! deployment configuration for the product and delivery option"
//! (`smithy_query(query="doc",shape="StartDeploymentConfigurationJob")`). So two
//! records live here and one is derived from the other:
//!
//! * [`DeploymentConfigurationJobRecord`] — one per started job, keyed by the
//!   minted `jobId`. It carries the configuration the job was started with, the
//!   status it is in, and the instant the clock makes its terminal status due
//!   ([`JOB_SETTLE_DELAY`]). The job's OUTCOME is decided when it is started, from
//!   the configuration itself ([`judge_configuration`]), and APPLIED when that
//!   instant has passed ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::settle_due_jobs`]):
//!   nothing is slept for, and every operation that resolves a job or a
//!   configuration materialises the transition the clock already owed before it
//!   reports anything.
//! * [`DeploymentConfigurationRecord`] — the ACTIVE configuration of one
//!   `(catalog, productId, deliveryOptionType)` resource, written by the job that
//!   succeeded and read by `GetDeploymentConfiguration`. Its ARN is the resource
//!   ARN the model's own `aws.api#arn` template states for the
//!   `DeploymentConfiguration` resource ([`deployment_configuration_arn`]), which
//!   is what `GetDeploymentConfigurationJob` reports as `configurationArn` and
//!   what the tagging operations address the configuration by.
//!
//! A job that CANNOT be published fails rather than being refused: the model's
//! `DeploymentConfigurationJobErrorCode$ENDPOINT_INVALID` documents a fulfillment
//! URL that "isn't a valid absolute HTTPS URL, or … points to a network location
//! that AWS Marketplace doesn't allow, such as a private or loopback address" as a
//! JOB error code, and `DeploymentConfigurationFulfillmentUrl` carries no
//! `@pattern` for the modelled walk to refuse it with — so such a configuration is
//! accepted, the job reaches `FAILED` with that code, and the active configuration
//! does not change. The model's other error code
//! (`FULFILLMENT_URL_UNREACHABLE`) is about what a request to the URL answered,
//! which this emulator makes no outbound call to find out, so no job is failed with
//! it.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use url::{Host, Url};

use crate::ops::types::{DeploymentConfigurationDetail, ValidationException};

use super::{ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState, ARN_NAMESPACE};

/// The delivery option type of the Data as a Service delivery option: the one
/// value the modelled `DeliveryOptionType` enum declares
/// (`smithy_query(query="shape",shape="DeliveryOptionType")`), which is also the
/// member of `DeploymentConfigurationDetail` that carries its settings.
///
/// Stated once here because it is the pairing [`delivery_option_type_of`] decides:
/// "the populated member of configuration must match the deliveryOptionType in the
/// request path".
pub const DELIVERY_OPTION_DATA_AS_A_SERVICE: &str = "data-as-a-service";

/// The status of a job that is still running, as the modelled
/// `DeploymentConfigurationJobStatus` spells it. `StartDeploymentConfigurationJob`
/// documents its own answer's status as this value.
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The status of a job whose configuration is now the active one.
pub const JOB_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The status of a job that did not publish its configuration. The active
/// deployment configuration did not change.
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// The modelled `DeploymentConfigurationJobErrorCode` value for a fulfillment URL
/// the service will not publish ([`is_publishable_fulfillment_url`]).
pub const JOB_ERROR_ENDPOINT_INVALID: &str = "ENDPOINT_INVALID";

/// The `message` a job failed with [`JOB_ERROR_ENDPOINT_INVALID`] carries: the
/// condition that error code is documented for, as the model words it
/// (`smithy_query(query="doc",shape="DeploymentConfigurationJobErrorCode",member="ENDPOINT_INVALID")`),
/// without its "start a new job" remedy — the remedy belongs to the documentation
/// and the message describes why the job failed
/// (`GetDeploymentConfigurationJobResponse$message`).
///
/// It carries nothing from the request: the URL is the caller's own value and the
/// code already says which member it is about.
pub const ENDPOINT_INVALID_MESSAGE: &str = "The fulfillment URL isn't a valid absolute HTTPS URL, \
     or it points to a network location that AWS Marketplace doesn't allow, such as a private or \
     loopback address.";

/// The scheme a fulfillment URL must use: `DataAsAServiceConfigurationDetail$fulfillmentUrl`
/// is "the HTTPS URL that AWS Marketplace redirects buyers to … The value must be a
/// valid URI that uses the HTTPS scheme".
pub const FULFILLMENT_URL_SCHEME: &str = "https";

/// The host name that names the local machine, which a fulfillment URL may not
/// point at — the name form of the loopback address
/// [`JOB_ERROR_ENDPOINT_INVALID`] names.
const LOOPBACK_HOST_NAME: &str = "localhost";

/// How long a deployment configuration job runs before the clock makes its terminal
/// status due.
///
/// The job "runs asynchronously", so a caller that reads it straight back sees the
/// `IN_PROGRESS` status the start answered with, and the terminal status becomes due
/// a span later. The span is this emulator's, not an AWS figure: it exists so the
/// transient status a client polls for is observable, and it is materialised from the
/// clock ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::settle_due_jobs`])
/// rather than waited for, so nothing sleeps.
pub const JOB_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// How many deployment configuration jobs this emulator holds at once.
///
/// An EMULATOR ceiling, not an AWS quota, and `StartDeploymentConfigurationJob`
/// models no quota error — so the ceiling is an EVICTION of the oldest job
/// ([`ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::record_configuration_job`])
/// and never a refusal: the jobs are a history, the one a start just recorded is the
/// newest, and `ListDeploymentConfigurationJobs` reports them newest first, so the
/// entry that falls off the end is the one furthest past. The ACTIVE configuration a
/// job published is a record of its own and outlives the job that wrote it.
pub const MAX_DEPLOYMENT_CONFIGURATION_JOBS: usize = 200;

/// The largest page `ListDeploymentConfigurationJobs` answers with: the maximum of
/// the modelled `DeploymentConfigurationMaxResults` `range 1..100`
/// (`smithy_query(query="shape",shape="DeploymentConfigurationMaxResults")`), which
/// the member's own doc restates as "valid values are 1 through 100".
pub const MAX_RESULTS_MAX: usize = 100;

/// The page size that member's modelled `@default` declares, which its doc restates
/// as "the default is 20".
pub const MAX_RESULTS_DEFAULT: usize = 20;

/// How a deployment configuration is keyed: `(catalog, productId, deliveryOptionType)`,
/// the three identifiers the model gives the `DeploymentConfiguration` resource
/// (`smithy_query(query="shape",shape="DeploymentConfiguration")`).
pub type ConfigurationKey = (String, String, String);

/// The ARN of the deployment configuration `(catalog, product_id,
/// delivery_option_type)` in `region`.
///
/// The resource portion is the one the MODEL states: the `DeploymentConfiguration`
/// resource's `aws.api#arn` template is
/// `catalogs/{catalog}/products/{productId}/delivery-options/{deliveryOptionType}`
/// (`smithy_query(query="shape",shape="DeploymentConfiguration")`), so the ARN of a
/// configuration is determined by the resource it names and is not minted from an
/// identifier of this emulator's own — unlike a deployment parameter's ARN
/// ([`super::deployment_parameter_arn`]), whose resource has an id.
pub fn deployment_configuration_arn(
    region: &str,
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "catalogs/{catalog}/products/{product_id}/delivery-options/{delivery_option_type}"
        ),
    )
}

/// What a started job will do once the clock makes its terminal status due, decided
/// when the job is started ([`judge_configuration`]) and applied unchanged.
///
/// Decided at the START because that is what the job is about: the configuration it
/// carries is the whole input of the publish, so the answer cannot depend on when
/// anybody looks — and a job that failed "fails again" for the same configuration,
/// as `ENDPOINT_INVALID` documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// The configuration becomes the active deployment configuration of its
    /// resource.
    Succeeded,
    /// The job fails with this modelled error code and message, and the active
    /// deployment configuration does not change.
    Failed {
        /// The modelled `DeploymentConfigurationJobErrorCode` value.
        error_code: &'static str,
        /// The sentence describing why the job failed.
        message: &'static str,
    },
}

impl JobOutcome {
    /// The modelled status a job with this outcome reaches.
    pub fn status(self) -> &'static str {
        match self {
            Self::Succeeded => JOB_STATUS_SUCCEEDED,
            Self::Failed { .. } => JOB_STATUS_FAILED,
        }
    }
}

/// One deployment configuration job, as `StartDeploymentConfigurationJob` recorded
/// it.
#[derive(Debug, Clone)]
pub struct DeploymentConfigurationJobRecord {
    /// The minted job id, matching the modelled `DeploymentConfigurationJobId`
    /// (`^[a-z0-9]+$` within `length 1..64`).
    pub job_id: String,
    /// The catalog the job was started in.
    pub catalog: String,
    /// The product the job publishes a configuration for.
    pub product_id: String,
    /// The delivery option the configuration is for.
    pub delivery_option_type: String,
    /// The region the starting request was signed for, so the ARN the published
    /// configuration carries is the same one however late it is reported.
    pub region: String,
    /// The configuration the job was started with — reported back from here, never
    /// echoed from a later request.
    pub configuration: DeploymentConfigurationDetail,
    /// The tags the start carried, applied to the configuration the job publishes.
    /// Only a start that CREATES the configuration may carry them.
    pub tags: BTreeMap<String, String>,
    /// The `clientToken` the start carried, if any: the `@idempotencyToken` a
    /// retry of the same request replays this job through.
    pub client_token: Option<String>,
    /// The modelled status, `IN_PROGRESS` until the clock makes [`Self::outcome`]
    /// due.
    pub status: String,
    /// What the job does once that instant has passed.
    pub outcome: JobOutcome,
    /// When the job was created.
    pub created_at: SystemTime,
    /// When its terminal status becomes due: [`JOB_SETTLE_DELAY`] after the start.
    pub settles_at: SystemTime,
    /// When it reached the `SUCCEEDED` or `FAILED` status — the instant that
    /// transition was owed, not the instant somebody looked.
    pub ended_at: Option<SystemTime>,
}

impl DeploymentConfigurationJobRecord {
    /// The configuration this job publishes, as [`ConfigurationKey`] keys it.
    pub fn key(&self) -> ConfigurationKey {
        (
            self.catalog.clone(),
            self.product_id.clone(),
            self.delivery_option_type.clone(),
        )
    }

    /// Whether this job is still running.
    pub fn is_in_progress(&self) -> bool {
        self.status == JOB_STATUS_IN_PROGRESS
    }

    /// Whether this job has succeeded, which is what makes its configuration the
    /// active one and gives it a `configurationArn` to report.
    pub fn has_succeeded(&self) -> bool {
        self.status == JOB_STATUS_SUCCEEDED
    }

    /// The error code this job failed with, which the model reports "only when the
    /// job status is FAILED" — so a job still running, or one that succeeded,
    /// reports none however its outcome was decided.
    pub fn error_code(&self) -> Option<&'static str> {
        match self.outcome {
            JobOutcome::Failed { error_code, .. } if self.status == JOB_STATUS_FAILED => {
                Some(error_code)
            }
            _ => None,
        }
    }

    /// The message describing why this job failed, present on the same condition
    /// as [`Self::error_code`].
    pub fn message(&self) -> Option<&'static str> {
        match self.outcome {
            JobOutcome::Failed { message, .. } if self.status == JOB_STATUS_FAILED => Some(message),
            _ => None,
        }
    }
}

/// The ACTIVE deployment configuration of one `(catalog, productId,
/// deliveryOptionType)` resource: "the configuration from the most recent
/// successful deployment configuration job"
/// (`smithy_query(query="doc",shape="GetDeploymentConfiguration")`).
#[derive(Debug, Clone)]
pub struct DeploymentConfigurationRecord {
    /// The resource ARN of this configuration ([`deployment_configuration_arn`]):
    /// what a succeeded job reports as `configurationArn`, and what the tagging
    /// operations address it by.
    pub configuration_arn: String,
    /// The catalog this configuration lives in.
    pub catalog: String,
    /// The product it belongs to.
    pub product_id: String,
    /// The delivery option it configures.
    pub delivery_option_type: String,
    /// The active configuration itself, as the job that published it carried it.
    pub configuration: DeploymentConfigurationDetail,
    /// The configuration's tags: the set the START that created it carried, as
    /// `TagResource` and `UntagResource` have left it since.
    pub tags: BTreeMap<String, String>,
    /// When the configuration was first published.
    pub created_at: SystemTime,
    /// When it was last written — by a later job publishing over it, or by a
    /// tagging write.
    pub updated_at: SystemTime,
}

impl DeploymentConfigurationRecord {
    /// The key this configuration is stored under.
    pub fn key(&self) -> ConfigurationKey {
        (
            self.catalog.clone(),
            self.product_id.clone(),
            self.delivery_option_type.clone(),
        )
    }

    /// The tag map as the modelled `TagsMap` / `Tags` wire shape.
    pub fn tags_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

impl ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState {
    /// Mint the next job id.
    ///
    /// 32 hexadecimal digits of `^[a-z0-9]+$`, inside the modelled
    /// `DeploymentConfigurationJobId` `length 1..64`. The counter is the store's,
    /// so an id is unique and a later job's id sorts after an earlier one's.
    pub fn mint_job_id(&mut self) -> String {
        self.next_job_id += 1;
        format!("{:032x}", self.next_job_id)
    }

    /// Record `job` as started, evicting the oldest job once the history is at
    /// [`MAX_DEPLOYMENT_CONFIGURATION_JOBS`] entries.
    pub fn record_configuration_job(&mut self, job: DeploymentConfigurationJobRecord) {
        self.configuration_jobs.insert(job.job_id.clone(), job);
        while self.configuration_jobs.len() > MAX_DEPLOYMENT_CONFIGURATION_JOBS {
            let oldest = self
                .configuration_jobs
                .values()
                .min_by_key(|held| (held.created_at, held.job_id.clone()))
                .map(|held| held.job_id.clone());
            match oldest {
                Some(job_id) => {
                    self.configuration_jobs.remove(&job_id);
                }
                None => break,
            }
        }
    }

    /// The job `job_id` names, whatever product it belongs to.
    pub fn configuration_job(&self, job_id: &str) -> Option<&DeploymentConfigurationJobRecord> {
        self.configuration_jobs.get(job_id)
    }

    /// The job a `clientToken` already started, for the `@idempotencyToken` replay.
    ///
    /// Resolved by reading the token off the JOBS rather than from a ledger of its
    /// own: a token identifies the job it started, the history is bounded
    /// ([`MAX_DEPLOYMENT_CONFIGURATION_JOBS`]), and keeping it on the record means a
    /// job evicted from the history takes its token with it instead of leaving an
    /// entry that could only replay a job that is gone.
    pub fn configuration_job_for_client_token(
        &self,
        client_token: &str,
    ) -> Option<&DeploymentConfigurationJobRecord> {
        self.configuration_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(client_token))
    }

    /// Every job started for `key`, ordered NEWEST FIRST — the order
    /// `ListDeploymentConfigurationJobs` documents.
    ///
    /// Two jobs of the same instant are ordered by their minted id, which the store's
    /// own counter makes increasing, so the order is total and stable across calls.
    pub fn configuration_jobs_newest_first(
        &self,
        key: &ConfigurationKey,
    ) -> Vec<&DeploymentConfigurationJobRecord> {
        let mut jobs: Vec<&DeploymentConfigurationJobRecord> = self
            .configuration_jobs
            .values()
            .filter(|job| &job.key() == key)
            .collect();
        jobs.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.job_id.cmp(&left.job_id))
        });
        jobs
    }

    /// The active deployment configuration of `key`, when a job has published one.
    pub fn active_configuration(&self, key: &ConfigurationKey) -> Option<&DeploymentConfigurationRecord> {
        self.by_configuration
            .get(key)
            .and_then(|arn| self.configurations.get(arn))
    }

    /// Whether this account holds a deployment configuration for `key` at all —
    /// which is what decides whether a start may carry `tags`: "only the request
    /// that creates the deployment configuration can apply tags".
    pub fn holds_configuration(&self, key: &ConfigurationKey) -> bool {
        self.active_configuration(key).is_some()
    }

    /// Materialise the job transitions the clock has already made due at `now`: every
    /// job still `IN_PROGRESS` whose [`DeploymentConfigurationJobRecord::settles_at`]
    /// has passed reaches the status its outcome names, and a job that SUCCEEDED
    /// publishes its configuration.
    ///
    /// The transition is owed at that instant whether or not anybody called — the job
    /// "runs asynchronously" — which is why a reporting operation may apply it, and
    /// why `endedAt` is the instant it was DUE rather than the instant a caller
    /// happened to look: two callers reading the same job must not see it end at two
    /// different times.
    ///
    /// Every operation that resolves a job, a configuration or a configuration's tags
    /// runs this first, so no caller sees a job whose terminal status the clock has
    /// already made due, and none sees an active configuration a succeeded job has
    /// not yet published.
    pub fn settle_due_jobs(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .configuration_jobs
            .values()
            .filter(|job| job.is_in_progress() && job.settles_at <= now)
            .map(|job| job.job_id.clone())
            .collect();
        for job_id in due {
            let Some(job) = self.configuration_jobs.get_mut(&job_id) else {
                continue;
            };
            job.status = job.outcome.status().to_string();
            job.ended_at = Some(job.settles_at);
            if job.outcome != JobOutcome::Succeeded {
                continue;
            }
            let published = job.clone();
            self.publish_configuration(&published);
        }
    }

    /// Make `job`'s configuration the active one of its resource, at the instant the
    /// job ended.
    ///
    /// A resource that already holds a configuration keeps its ARN and its tags — it
    /// is the same resource, and tags are changed through `TagResource` /
    /// `UntagResource` — and takes the new configuration; one that holds none is
    /// created, with the tags the START carried.
    fn publish_configuration(&mut self, job: &DeploymentConfigurationJobRecord) {
        let at = job.ended_at.unwrap_or(job.settles_at);
        let key = job.key();
        if let Some(arn) = self.by_configuration.get(&key).cloned() {
            if let Some(record) = self.configurations.get_mut(&arn) {
                record.configuration = job.configuration.clone();
                record.updated_at = at;
                return;
            }
        }
        let configuration_arn = deployment_configuration_arn(
            &job.region,
            &job.catalog,
            &job.product_id,
            &job.delivery_option_type,
        );
        self.configurations.insert(
            configuration_arn.clone(),
            DeploymentConfigurationRecord {
                configuration_arn: configuration_arn.clone(),
                catalog: job.catalog.clone(),
                product_id: job.product_id.clone(),
                delivery_option_type: job.delivery_option_type.clone(),
                configuration: job.configuration.clone(),
                tags: job.tags.clone(),
                created_at: at,
                updated_at: at,
            },
        );
        self.by_configuration.insert(key, configuration_arn);
    }
}

/// The delivery option type the POPULATED member of `configuration` is the
/// configuration of.
///
/// `DeploymentConfigurationDetail` is a union whose "exactly one member must be
/// set, and it must match the delivery option type"
/// (`smithy_query(query="shape",shape="DeploymentConfigurationDetail")`), so the
/// pairing between a union member and a `DeliveryOptionType` value is a fact of the
/// model and is stated here once. The match is exhaustive, so a member the model
/// adds later cannot be left unpaired.
pub fn delivery_option_type_of(configuration: &DeploymentConfigurationDetail) -> &'static str {
    match configuration {
        DeploymentConfigurationDetail::dataAsAService(_) => DELIVERY_OPTION_DATA_AS_A_SERVICE,
    }
}

/// The configuration's content as one comparable string, for deciding whether a
/// request carrying a `clientToken` is the SAME request as the one that token
/// already started.
///
/// It names the populated member and the settings that member carries, so two
/// configurations compare equal exactly when they would publish the same thing.
pub fn configuration_fingerprint(configuration: &DeploymentConfigurationDetail) -> String {
    match configuration {
        DeploymentConfigurationDetail::dataAsAService(detail) => format!(
            "{DELIVERY_OPTION_DATA_AS_A_SERVICE}:{}",
            detail.fulfillment_url
        ),
    }
}

/// What a job started with `configuration` will do: publish it, or fail with the
/// modelled error code for the reason it cannot be published.
///
/// The only publishable-or-not judgement the model states is about the fulfillment
/// URL ([`is_publishable_fulfillment_url`]), so every other configuration succeeds.
pub fn judge_configuration(configuration: &DeploymentConfigurationDetail) -> JobOutcome {
    match configuration {
        DeploymentConfigurationDetail::dataAsAService(detail) => {
            if is_publishable_fulfillment_url(&detail.fulfillment_url) {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed {
                    error_code: JOB_ERROR_ENDPOINT_INVALID,
                    message: ENDPOINT_INVALID_MESSAGE,
                }
            }
        }
    }
}

/// Whether `candidate` is a fulfillment URL this service publishes.
///
/// Two conditions, both read off the model: the member is "the HTTPS URL that AWS
/// Marketplace redirects buyers to … The value must be a valid URI that uses the
/// HTTPS scheme", and the `ENDPOINT_INVALID` error code is documented for a URL
/// that "isn't a valid absolute HTTPS URL, or … points to a network location that
/// AWS Marketplace doesn't allow, such as a private or loopback address". So:
///
/// 1. it parses as an absolute URL whose scheme is [`FULFILLMENT_URL_SCHEME`];
/// 2. its host is a network location a buyer's browser can be redirected to —
///    neither the local machine nor an address inside a private network
///    ([`is_disallowed_host`]).
///
/// The parse is the `url` crate's (RFC 3986 / WHATWG), so what counts as a valid
/// absolute URL is decided by a maintained parser rather than by a second,
/// hand-written reading of the grammar.
fn is_publishable_fulfillment_url(candidate: &str) -> bool {
    let Ok(parsed) = Url::parse(candidate) else {
        return false;
    };
    if parsed.scheme() != FULFILLMENT_URL_SCHEME {
        return false;
    }
    match parsed.host() {
        Some(host) => !is_disallowed_host(&host),
        None => false,
    }
}

/// Whether `host` is a network location a published fulfillment URL may not point
/// at: the local machine, or an address that is only reachable inside one network.
///
/// An IP literal is judged by what the address IS — loopback, unspecified, a
/// private (RFC 1918) or link-local IPv4 address, an IPv6 unique-local or
/// link-local address — and a NAME by whether it names the local machine, which is
/// the name form of the same refusal. Any other name is a public DNS name as far as
/// this service can tell, and is published.
fn is_disallowed_host(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let lowered = name.to_ascii_lowercase();
            lowered == LOOPBACK_HOST_NAME
                || lowered.ends_with(&format!(".{LOOPBACK_HOST_NAME}"))
        }
        Host::Ipv4(address) => is_disallowed_address(IpAddr::V4(*address)),
        Host::Ipv6(address) => is_disallowed_address(IpAddr::V6(*address)),
    }
}

/// Whether `address` is one of the network locations above, as an IP address.
///
/// The IPv6 cases are spelled from the address's own leading bits — unique-local is
/// `fc00::/7` and link-local `fe80::/10` — because the standard library's
/// predicates for them are not available on a stable toolchain; the IPv4 ones are
/// the standard library's own.
fn is_disallowed_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_unspecified() || v4.is_private() || v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            let leading = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || leading & 0xfe00 == 0xfc00
                || leading & 0xffc0 == 0xfe80
        }
    }
}

/// The modelled `ValidationException` (HTTP 400) for a `configuration` whose
/// populated member is not the configuration of the `deliveryOptionType` the
/// request's path names.
///
/// The rule is the model's: "the populated member of configuration must match the
/// deliveryOptionType in the request path". The sentence names both values, which
/// is what a caller has to change, and blames the member that carried the
/// mismatch.
pub fn configuration_delivery_option_mismatch(
    path_delivery_option_type: &str,
    populated_delivery_option_type: &str,
) -> ValidationException {
    ValidationException {
        message: format!(
            "Configuration: The populated member of configuration is the configuration of \
             delivery option type {populated_delivery_option_type}, which does not match the \
             deliveryOptionType {path_delivery_option_type} in the request path."
        ),
        field_name: "configuration".to_string(),
    }
}

/// The modelled `ValidationException` (HTTP 400) for a start that carries `tags`
/// for a deployment configuration that already exists.
///
/// The rule is the model's own:
/// `StartDeploymentConfigurationJobRequest$tags` documents that "only the request
/// that creates the deployment configuration can apply tags. If the deployment
/// configuration already exists, a request that includes one or more tags fails
/// with a ValidationException. To change tags on an existing deployment
/// configuration, use TagResource and UntagResource."
pub fn tags_on_existing_configuration() -> ValidationException {
    ValidationException {
        message: "Tags: Only the request that creates the deployment configuration can apply \
                  tags. To change tags on an existing deployment configuration, use TagResource \
                  and UntagResource."
            .to_string(),
        field_name: "tags".to_string(),
    }
}

/// The modelled `ValidationException` (HTTP 400) for a `nextToken` that is not one
/// this service handed out for the page sequence being read.
///
/// The member is "the token for the next page of results. Use the value returned in
/// the previous response", so a value of any other provenance names no page.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: "NextToken: The provided NextToken is invalid. Use the nextToken value returned \
                  in the previous response."
            .to_string(),
        field_name: "nextToken".to_string(),
    }
}
