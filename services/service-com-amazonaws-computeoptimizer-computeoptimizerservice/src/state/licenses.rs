// SPDX-License-Identifier: CC0-1.0

//! The commercial software licenses Compute Optimizer observes on Amazon EC2 instances, and
//! the license recommendation a reader derives from that observation.
//!
//! Part of [`super`]'s one state module, in its own file so one concern lives in one place.

use super::*;

/// The `LicenseFinding` value outside the service-wide `Finding` vocabulary
/// ([`LICENSE_FINDING_VALUES`]): "InsufficentMetrics — When Compute Optimizer detects that
/// your CloudWatch Application Insights isn't enabled or is enabled with insufficient
/// permissions" (`LicenseRecommendation$finding`). Its other two values are
/// [`FINDING_OPTIMIZED`] and [`FINDING_NOT_OPTIMIZED`].
pub const LICENSE_FINDING_INSUFFICIENT_METRICS: &str = "InsufficientMetrics";

/// The `LicenseFindingReasonCode` values ([`LICENSE_FINDING_REASON_CODE_VALUES`]), each as
/// `LicenseRecommendation$findingReasonCodes` documents it: `Optimized` — "All
/// specifications of your license meet the performance requirements of your workload";
/// `LicenseOverprovisioned` — "your license can be downgraded while still meeting the
/// performance requirements of your workload"; `InvalidCloudWatchApplicationInsightsSetup`
/// — "CloudWatch Application Insights isn't configured properly";
/// `CloudWatchApplicationInsightsError` — "There is a CloudWatch Application Insights
/// error".
pub const LICENSE_REASON_OPTIMIZED: &str = "Optimized";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_OVERPROVISIONED: &str = "LicenseOverprovisioned";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_INVALID_INSIGHTS_SETUP: &str =
    "InvalidCloudWatchApplicationInsightsSetup";
/// See [`LICENSE_REASON_OPTIMIZED`].
pub const LICENSE_REASON_INSIGHTS_ERROR: &str = "CloudWatchApplicationInsightsError";

/// The `LicenseRecommendationFilterName` values
/// ([`LICENSE_RECOMMENDATION_FILTER_NAME_VALUES`]), named so the filter rules below read as
/// the model's own vocabulary.
pub const LICENSE_FILTER_FINDING: &str = "Finding";
/// See [`LICENSE_FILTER_FINDING`].
pub const LICENSE_FILTER_FINDING_REASON_CODE: &str = "FindingReasonCode";
/// See [`LICENSE_FILTER_FINDING`].
pub const LICENSE_FILTER_LICENSE_NAME: &str = "LicenseName";

/// The `LicenseEdition` values ([`LICENSE_EDITION_VALUES`]).
pub const LICENSE_EDITION_ENTERPRISE: &str = "Enterprise";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_STANDARD: &str = "Standard";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_FREE: &str = "Free";
/// See [`LICENSE_EDITION_ENTERPRISE`].
pub const LICENSE_EDITION_NOT_FOUND: &str = "NoLicenseEditionFound";

/// The `LicenseModel` values ([`LICENSE_MODEL_VALUES`]).
pub const LICENSE_MODEL_LICENSE_INCLUDED: &str = "LicenseIncluded";
/// See [`LICENSE_MODEL_LICENSE_INCLUDED`].
pub const LICENSE_MODEL_BRING_YOUR_OWN: &str = "BringYourOwnLicense";

/// The one `LicenseName` the model declares ([`LICENSE_NAME_VALUES`]): the commercial
/// software whose licenses this service analyses.
pub const LICENSE_NAME_SQL_SERVER: &str = "SQLServer";

/// The one `MetricSourceProvider` the model declares ([`METRIC_SOURCE_PROVIDER_VALUES`]) —
/// the source "required to generate recommendations for commercial software licenses"
/// (`LicenseConfiguration$metricsSource`).
pub const METRIC_SOURCE_APPLICATION_INSIGHTS: &str = "CloudWatchApplicationInsights";

/// The service namespace of a CloudWatch Application Insights application, which is what a
/// license's metric source names in its `providerArn`.
pub const APPLICATION_INSIGHTS_ARN_SERVICE: &str = "applicationinsights";

/// How much of a licensed product's feature set an edition licenses, as a rank: a workload
/// that stays inside a lower-ranked edition's features runs on that edition, so a license
/// of a higher-ranked edition than its workload needs "can be downgraded while still
/// meeting the performance requirements of your workload"
/// (`LicenseRecommendation$findingReasonCodes`). `NoLicenseEditionFound` names no edition at
/// all, so it has no rank and nothing is recommended for it.
pub fn license_edition_rank(edition: &str) -> Option<u8> {
    match edition {
        LICENSE_EDITION_FREE => Some(0),
        LICENSE_EDITION_STANDARD => Some(1),
        LICENSE_EDITION_ENTERPRISE => Some(2),
        _ => None,
    }
}

/// What one core of a LICENSE-INCLUDED license of `license_name` at `edition` costs per
/// month, in USD, or `None` for a licensed product or edition this emulator holds no price
/// for — whose recommendation then carries no savings figure rather than an invented one.
///
/// AWS publishes no license price list through this API, so this is the emulator's OWN
/// per-core-month table, stated once: every license cost and saving this service reports is
/// computed from it and the instance's core count
/// ([`ObservedLicensedInstance::license_monthly_cost`]), so the figures of one license
/// recommendation agree with each other and with the summary that aggregates it. The `Free`
/// edition is the one that carries no license charge at all, which is why it costs nothing.
pub fn license_core_monthly_price(license_name: &str, edition: &str) -> Option<f64> {
    let per_core_hour = match (license_name, edition) {
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_ENTERPRISE) => 0.40,
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_STANDARD) => 0.11,
        (LICENSE_NAME_SQL_SERVER, LICENSE_EDITION_FREE) => 0.0,
        _ => return None,
    };
    Some(round_hundredths(per_core_hour * HOURS_PER_MONTH))
}

/// What CloudWatch Application Insights — the one metric source a license recommendation is
/// generated from — reports for one licensed instance.
///
/// The three states are the ones `LicenseRecommendation$finding` and its reason codes
/// distinguish: a source that is reporting (so the analysis can reach a finding about the
/// LICENSE), one that "isn't enabled or is enabled with insufficient permissions" (an
/// invalid setup), and one that is answering with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationInsights {
    /// Enabled, permitted, and reporting the license metrics the analysis reads.
    Reporting,
    /// Not enabled, or enabled with insufficient permissions.
    InvalidSetup,
    /// Enabled, but the metric source is answering with an error.
    Failing,
}

/// What adopting a license recommendation is worth, as
/// [`ObservedLicensedInstance::savings`] computes it: NAMED members rather than a pair, so a
/// caller cannot read the percentage as the amount or the other way round.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LicenseSavings {
    /// The saving "as a percentage of monthly cost"
    /// (`SavingsOpportunity$savingsOpportunityPercentage`).
    pub percentage: f64,
    /// What the move saves per month, in [`CURRENCY_USD`].
    pub monthly_amount: f64,
}

/// One Amazon EC2 instance that runs on a commercial software license, as Compute Optimizer
/// observes it: the license it runs under, the metric source the analysis reads, and the
/// edition of that product's feature set the workload was observed USING. This is the
/// OBSERVATION; everything a `LicenseRecommendation` reports is derived from it, so no two
/// readers of the same license can disagree.
#[derive(Debug, Clone)]
pub struct ObservedLicensedInstance {
    /// The EC2 instance id, which is this instance's key in the inventory. The ARN a reader
    /// reports is built from it and the region the request was signed for
    /// ([`Self::instance_arn`]), because Compute Optimizer is a REGIONAL service: the
    /// licenses it reports are the ones in the Region the caller addressed.
    pub instance_id: String,
    /// The account that owns the instance, and so "The Amazon Web Services account ID of the
    /// license" (`LicenseRecommendation$accountId`).
    pub account_id: String,
    /// "The instance type used in the license" (`LicenseConfiguration$instanceType`).
    pub instance_type: String,
    /// "The current number of cores associated with the instance"
    /// (`LicenseConfiguration$numberOfCores`) — what a per-core license is charged on.
    pub number_of_cores: i32,
    /// "The operating system of the instance" (`LicenseConfiguration$operatingSystem`).
    pub operating_system: String,
    /// The `LicenseName` of the product the instance runs.
    pub license_name: String,
    /// The `LicenseEdition` the instance is licensed for today.
    pub license_edition: String,
    /// The `LicenseModel` the license is held under: a license AWS includes in the
    /// instance's price, or one the customer brings.
    pub license_model: String,
    /// "The version of the license for the application that runs on the instance"
    /// (`LicenseConfiguration$licenseVersion`).
    pub license_version: String,
    /// What the metric source reports for this instance, which is what decides whether the
    /// analysis can reach a finding about the license at all.
    pub application_insights: ApplicationInsights,
    /// The resource group of the CloudWatch Application Insights application that watches
    /// the instance, from which [`Self::metrics_source`] builds the metric source's
    /// `providerArn`. `None` for an instance no application watches, which has no metric
    /// source to report.
    pub application_insights_resource_group: Option<String>,
    /// The LOWEST `LicenseEdition` whose licensed feature set covers everything the
    /// workload was observed using — what the metric source reports about the license.
    /// Compared against [`Self::license_edition`], this is what makes a license
    /// over-provisioned or optimized.
    pub edition_in_use: String,
    /// How many days Compute Optimizer has been observing this instance's license. A
    /// look-back preference longer than this cannot be honoured, so the analysed period is
    /// the shorter of the two.
    pub days_observed: f64,
    /// "A list of tags assigned to an EC2 instance" (`LicenseRecommendation$tags`), as
    /// key/value pairs in the order EC2 reports them.
    pub tags: Vec<(String, String)>,
}

impl ObservedLicensedInstance {
    /// This instance's identity WITHOUT the region — the resource part of its ARN.
    pub fn arn_resource(&self) -> String {
        format!("instance/{}", self.instance_id)
    }

    /// This instance's ARN in `region`: "arn:aws:ec2:region:aws_account_id:instance/
    /// instance-id" (`GetLicenseRecommendations$resourceArns`).
    ///
    /// Built through [`engine::emulator_arn`], the one ARN construction path: this emulator
    /// presents exactly one account ([`engine::emulator_account_id`]) and answers for no
    /// other ([`requested_account_id`]), so every observation it holds is that account's.
    pub fn instance_arn(&self, region: &str) -> String {
        debug_assert_eq!(
            self.account_id,
            ::engine::emulator_account_id(),
            "the license inventory holds the emulator account's instances only"
        );
        ::engine::emulator_arn("ec2", region, &self.arn_resource())
    }

    /// "The number of days for which utilization metrics were analyzed for an instance that
    /// runs on a license" (`LicenseRecommendation$lookbackPeriodInDays`): the preferred
    /// look-back period ([`look_back_period_in_days`]), or however long this instance has
    /// been observed when that is shorter.
    pub fn analysed_days(&self, look_back_period: &str) -> f64 {
        look_back_period_in_days(look_back_period).min(self.days_observed)
    }

    /// "The list of metric sources required to generate recommendations for commercial
    /// software licenses" (`LicenseConfiguration$metricsSource`): the CloudWatch Application
    /// Insights application that watches this instance, in the Region the request
    /// addressed, or `None` for an instance no application watches.
    pub fn metrics_source(&self, region: &str) -> Option<Vec<MetricSource>> {
        let resource_group = self.application_insights_resource_group.as_ref()?;
        let provider_arn = ::engine::emulator_arn(
            APPLICATION_INSIGHTS_ARN_SERVICE,
            region,
            &format!("application/resource-group/{resource_group}"),
        );
        Some(vec![MetricSource {
            provider: Some(METRIC_SOURCE_APPLICATION_INSIGHTS.to_string()),
            provider_arn: Some(provider_arn),
        }])
    }

    /// The `LicenseEdition` this analysis would move the license to, or `None` where it
    /// would leave the license as it is.
    ///
    /// The edition the workload was observed needing ([`Self::edition_in_use`]), and only
    /// when that edition licenses LESS than the one in force: a license is moved only
    /// downwards, because what Compute Optimizer identifies here is a license that "can be
    /// downgraded while still meeting the performance requirements of your workload". An
    /// instance whose metric source is not reporting has no observed feature set to compare,
    /// so nothing is recommended for it.
    pub fn recommended_license_edition(&self) -> Option<&str> {
        if self.application_insights != ApplicationInsights::Reporting {
            return None;
        }
        let current = license_edition_rank(&self.license_edition)?;
        let needed = license_edition_rank(&self.edition_in_use)?;
        (needed < current).then_some(self.edition_in_use.as_str())
    }

    /// The license's `LicenseFinding`: `InsufficientMetrics` while the metric source is not
    /// reporting ("your CloudWatch Application Insights isn't enabled or is enabled with
    /// insufficient permissions"), `NotOptimized` for a license the analysis would downgrade
    /// (the EC2 infrastructure "isn't using any of the … license features you're paying
    /// for"), and `Optimized` when "all specifications of your license meet the performance
    /// requirements of your workload".
    pub fn finding(&self) -> &'static str {
        if self.application_insights != ApplicationInsights::Reporting {
            return LICENSE_FINDING_INSUFFICIENT_METRICS;
        }
        if self.recommended_license_edition().is_some() {
            FINDING_NOT_OPTIMIZED
        } else {
            FINDING_OPTIMIZED
        }
    }

    /// "The reason for the finding classification for an instance that runs on a license"
    /// (`LicenseRecommendation$findingReasonCodes`): the `LicenseFindingReasonCode` behind
    /// [`Self::finding`] — which side of the metric source the finding came from, and, for a
    /// license the analysis did read, whether it can be downgraded.
    pub fn finding_reason_codes(&self) -> Vec<String> {
        let code = match self.application_insights {
            ApplicationInsights::InvalidSetup => LICENSE_REASON_INVALID_INSIGHTS_SETUP,
            ApplicationInsights::Failing => LICENSE_REASON_INSIGHTS_ERROR,
            ApplicationInsights::Reporting => {
                if self.recommended_license_edition().is_some() {
                    LICENSE_REASON_OVERPROVISIONED
                } else {
                    LICENSE_REASON_OPTIMIZED
                }
            }
        };
        vec![code.to_string()]
    }

    /// What this instance's license costs per month at `edition`, in USD, or `None` where
    /// this emulator holds no price for it: a license the customer BRINGS is not charged
    /// through AWS, and [`license_core_monthly_price`] holds no price for every licensed
    /// product or edition. A license with no price carries no savings figure rather than an
    /// invented one.
    pub fn license_monthly_cost(&self, edition: &str) -> Option<f64> {
        if self.license_model != LICENSE_MODEL_LICENSE_INCLUDED {
            return None;
        }
        let per_core = license_core_monthly_price(&self.license_name, edition)?;
        Some(round_hundredths(per_core * f64::from(self.number_of_cores.max(0))))
    }

    /// What adopting the recommended edition is worth: the saving as a percentage of monthly
    /// cost (`SavingsOpportunity$savingsOpportunityPercentage`) and the monthly amount, or
    /// `None` where there is no recommended edition or no price for one of the two editions.
    /// A move that costs no less saves nothing rather than a negative amount.
    pub fn savings(&self) -> Option<LicenseSavings> {
        let recommended = self.recommended_license_edition()?;
        let current_cost = self.license_monthly_cost(&self.license_edition)?;
        let recommended_cost = self.license_monthly_cost(recommended)?;
        let saved = round_hundredths((current_cost - recommended_cost).max(0.0));
        Some(LicenseSavings {
            percentage: percentage_of(saved, current_cost),
            monthly_amount: saved,
        })
    }

    /// "An array of objects that describe the license recommendation options"
    /// (`LicenseRecommendation$licenseRecommendationOptions`): the one option this analysis
    /// has — the edition it would move the license to, at the same operating system and
    /// license model, since neither of those is what the analysis found to change — or
    /// `None` for a license it would leave as it is, which has no option to report.
    pub fn license_recommendation_options(&self) -> Option<Vec<LicenseRecommendationOption>> {
        let recommended = self.recommended_license_edition()?;
        Some(vec![LicenseRecommendationOption {
            // One option, so it is the top one: "The top recommendation option is ranked as
            // 1" (`LicenseRecommendationOption$rank`).
            rank: 1,
            operating_system: Some(self.operating_system.clone()),
            license_edition: Some(recommended.to_string()),
            license_model: Some(self.license_model.clone()),
            savings_opportunity: self.savings().map(|savings| SavingsOpportunity {
                savings_opportunity_percentage: savings.percentage,
                estimated_monthly_savings: Some(EstimatedMonthlySavings {
                    currency: Some(CURRENCY_USD.to_string()),
                    value: savings.monthly_amount,
                }),
            }),
        }])
    }

    /// The instance's tags as the modelled `Tags` list, or `None` when it carries none.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        modelled_tags(&self.tags)
    }

    /// "An object that describes the current configuration of an instance that runs on a
    /// license" (`LicenseRecommendation$currentLicenseConfiguration`), as the endpoint in
    /// `region` reports it.
    pub fn current_license_configuration(&self, region: &str) -> LicenseConfiguration {
        LicenseConfiguration {
            number_of_cores: self.number_of_cores,
            instance_type: Some(self.instance_type.clone()),
            operating_system: Some(self.operating_system.clone()),
            license_edition: Some(self.license_edition.clone()),
            license_name: Some(self.license_name.clone()),
            license_model: Some(self.license_model.clone()),
            license_version: Some(self.license_version.clone()),
            metrics_source: self.metrics_source(region),
        }
    }

    /// The license recommendation this observation yields in `region` under
    /// `look_back_period`, as generated at `last_refresh_timestamp`.
    ///
    /// Every member is computed here from the stored observation: the configuration is the
    /// license in force, the finding and its reason code say what the metric source reported
    /// about it, the option is the edition the observed feature set asks for, and the saving
    /// is what that move is worth at [`license_core_monthly_price`].
    pub fn to_recommendation(
        &self,
        region: &str,
        look_back_period: &str,
        last_refresh_timestamp: Option<DateTime>,
    ) -> LicenseRecommendation {
        LicenseRecommendation {
            resource_arn: Some(self.instance_arn(region)),
            account_id: Some(self.account_id.clone()),
            current_license_configuration: Some(self.current_license_configuration(region)),
            lookback_period_in_days: self.analysed_days(look_back_period),
            last_refresh_timestamp,
            finding: Some(self.finding().to_string()),
            finding_reason_codes: Some(self.finding_reason_codes()),
            license_recommendation_options: self.license_recommendation_options(),
            tags: self.tag_list(),
        }
    }

    /// This licensed instance as a row of the findings catalog, i.e. as
    /// `GetRecommendationSummaries` aggregates it.
    ///
    /// The projection is what keeps one license's analysis in ONE place: the summary reader
    /// and `GetLicenseRecommendations` both answer from this observation, so they cannot
    /// disagree about a license's finding or what adopting its recommendation saves. A
    /// license whose metrics were insufficient is a row like any other, carrying the
    /// `InsufficientMetrics` classification that reader reports for it: a summary of the
    /// account's licenses counts a license at each of the three `LicenseFinding` values
    /// ([`findings_reported_for`]), so the count at that one has to have rows behind it.
    pub fn to_analysed_resource(&self) -> AnalysedResource {
        let finding = self.finding();
        let current_monthly_cost = self
            .license_monthly_cost(&self.license_edition)
            .unwrap_or_default();
        let recommended_monthly_cost = self
            .recommended_license_edition()
            .and_then(|edition| self.license_monthly_cost(edition))
            .unwrap_or(current_monthly_cost);
        AnalysedResource {
            account_id: self.account_id.clone(),
            resource_type: SOURCE_TYPE_LICENSE.to_string(),
            // The catalog records a resource's REGION-FREE identity
            // ([`AnalysedResource::arn_in`]), which for a license is the instance it runs on.
            arn_service: "ec2".to_string(),
            arn_resource: self.arn_resource(),
            finding: finding.to_string(),
            // A license's reason codes are `LicenseFindingReasonCode` values, and
            // `ReasonCodeSummary$name` carries only the two `FindingReasonCode` values
            // ([`METRIC_REASON_MEMORY_OVERPROVISIONED`]), so no license reason code is a
            // count a summary can report.
            finding_reason_codes: Vec::new(),
            // `LicenseRecommendation` declares no performance-risk member: the analysis
            // records nothing about the risk a license runs, so the row carries the lowest
            // rating rather than inventing a higher one.
            current_performance_risk: PERFORMANCE_RISK_VERY_LOW.to_string(),
            current_monthly_cost,
            recommended_monthly_cost,
            // A license recommendation infers no workload type (`LicenseRecommendation`
            // declares no such member).
            inferred_workload_types: Vec::new(),
            // A license is not a database, so there is no database configuration to record.
            rds_database: None,
        }
    }
}

/// Check a `LicenseRecommendationFilters` list against the vocabulary the model declares for
/// it, returning the reason it is not valid.
///
/// `name` is a `LicenseRecommendationFilterName` and the values a filter carries are the
/// value set of the member that name selects on: "If you specify the `name` parameter as
/// `Finding`, then specify `Optimized`, `NotOptimized`, or `InsufficentMetrics`"; "If you
/// specify the `name` parameter as `FindingReasonCode`, then specify `Optimized`,
/// `LicenseOverprovisioned`, `InvalidCloudwatchApplicationInsights`, or
/// `CloudwatchApplicationInsightsError`" — the model's own
/// [`LICENSE_FINDING_REASON_CODE_VALUES`] spell those last two
/// `InvalidCloudWatchApplicationInsightsSetup` and `CloudWatchApplicationInsightsError`, and
/// the model is what the wire carries. A `LicenseName` filter selects on
/// [`LICENSE_NAME_VALUES`]. A name or a value outside those sets is refused rather than
/// silently matching nothing — the same rule [`lambda_filter_rejection`] applies.
pub fn license_filter_rejection(filters: &[LicenseRecommendationFilter]) -> Option<String> {
    for filter in filters {
        let Some(name) = filter.name.as_deref() else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: a filter name must be \
                 supplied, one of [{}].",
                LICENSE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        let Some(vocabulary) = license_filter_vocabulary(name) else {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {name} is not a filter \
                 name, which must be one of [{}].",
                LICENSE_RECOMMENDATION_FILTER_NAME_VALUES.join(", ")
            ));
        };
        if let Some(value) = filter
            .values
            .iter()
            .flatten()
            .find(|value| !vocabulary.contains(&value.as_str()))
        {
            return Some(format!(
                "The value supplied for parameter filters is not valid: {value} is not a value \
                 of the {name} filter, which must be one of [{}].",
                vocabulary.join(", ")
            ));
        }
    }
    None
}

/// The value set a `LicenseRecommendationFilterName` selects on, or `None` for a name the
/// model does not declare.
pub(super) fn license_filter_vocabulary(name: &str) -> Option<&'static [&'static str]> {
    match name {
        LICENSE_FILTER_FINDING => Some(LICENSE_FINDING_VALUES),
        LICENSE_FILTER_FINDING_REASON_CODE => Some(LICENSE_FINDING_REASON_CODE_VALUES),
        LICENSE_FILTER_LICENSE_NAME => Some(LICENSE_NAME_VALUES),
        _ => None,
    }
}

/// Does `recommendation` satisfy every `GetLicenseRecommendations` filter?
///
/// A filter matches when the member its name selects on equals any of the filter's values —
/// for the reason codes, which are a LIST, when any reported code is among them. Several
/// filters are ANDed, and a filter carrying no values constrains nothing. A member the
/// recommendation does not carry matches nothing. An unrecognised name never reaches here:
/// [`license_filter_rejection`] refuses the request first.
pub fn license_recommendation_matches_filters(
    recommendation: &LicenseRecommendation,
    filters: &[LicenseRecommendationFilter],
) -> bool {
    filters.iter().all(|filter| {
        let Some(values) = filter.values.as_ref().filter(|values| !values.is_empty()) else {
            return true;
        };
        match filter.name.as_deref() {
            Some(LICENSE_FILTER_FINDING) => recommendation
                .finding
                .as_deref()
                .is_some_and(|finding| values.iter().any(|value| value == finding)),
            Some(LICENSE_FILTER_FINDING_REASON_CODE) => recommendation
                .finding_reason_codes
                .as_ref()
                .is_some_and(|codes| {
                    codes
                        .iter()
                        .any(|code| values.iter().any(|value| value == code))
                }),
            Some(LICENSE_FILTER_LICENSE_NAME) => recommendation
                .current_license_configuration
                .as_ref()
                .and_then(|configuration| configuration.license_name.as_deref())
                .is_some_and(|license_name| values.iter().any(|value| value == license_name)),
            _ => false,
        }
    })
}

/// The licensed Amazon EC2 instances this emulator observes, generated once when the store
/// is created.
///
/// The emulator watches no real workload, so WHICH instances run a commercial software
/// license, and what their metric source reports, is a fixed observation rather than
/// something derived from traffic. Everything a license recommendation reports is
/// nevertheless computed from what is stored here — the finding and its reason code from the
/// metric source and the observed feature set, the recommended edition from that feature
/// set, and every cost from [`license_core_monthly_price`] — so a reader cannot report a
/// figure the observation does not support.
///
/// The set spans the cases a license recommendation distinguishes: a license-included
/// Enterprise license whose workload stays inside Standard's features (so the move saves
/// money), one already at the edition its workload needs, a license the customer brings
/// (which AWS charges nothing for, so its downgrade carries no savings figure), an instance
/// whose metric source is not set up, and one whose metric source is answering with an
/// error.
pub(super) fn observed_licensed_instances() -> Vec<ObservedLicensedInstance> {
    let account_id = ::engine::emulator_account_id().to_string();
    let instance = |instance_id: &str,
                    instance_type: &str,
                    number_of_cores: i32,
                    license_edition: &str,
                    license_model: &str,
                    application_insights: ApplicationInsights,
                    application_insights_resource_group: Option<&str>,
                    edition_in_use: &str,
                    days_observed: f64,
                    tags: &[(&str, &str)]| ObservedLicensedInstance {
        instance_id: instance_id.to_string(),
        account_id: account_id.clone(),
        instance_type: instance_type.to_string(),
        number_of_cores,
        // The `OperatingSystem` member is a free-form string in the model; this is the
        // platform EC2 reports for a Windows instance that runs SQL Server.
        operating_system: "Windows".to_string(),
        license_name: LICENSE_NAME_SQL_SERVER.to_string(),
        license_edition: license_edition.to_string(),
        license_model: license_model.to_string(),
        license_version: "2019".to_string(),
        application_insights,
        application_insights_resource_group: application_insights_resource_group
            .map(ToString::to_string),
        edition_in_use: edition_in_use.to_string(),
        days_observed,
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    };

    vec![
        // Enterprise, license-included, on 16 cores — and the workload was never observed
        // using a feature outside Standard's set, so the license can be downgraded and the
        // move saves the difference between the two editions' per-core prices.
        instance(
            "i-0a1b2c3d4e5f70001",
            "r5.4xlarge",
            16,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Reporting,
            Some("sql-reporting"),
            LICENSE_EDITION_STANDARD,
            93.0,
            &[("Owner", "TeamA"), ("Environment", "production")],
        ),
        // Standard, license-included, running a workload that needs Standard: every
        // specification of the license meets the workload's requirements, so there is
        // nothing to move to. Observed for a week only, so that week is the period analysed
        // however long a look-back period the account prefers.
        instance(
            "i-0a1b2c3d4e5f70002",
            "m5.2xlarge",
            8,
            LICENSE_EDITION_STANDARD,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Reporting,
            Some("sql-orders"),
            LICENSE_EDITION_STANDARD,
            7.0,
            &[("Owner", "TeamB")],
        ),
        // Enterprise brought by the customer, on a workload that stays inside the free
        // edition: the license can be downgraded, but AWS charges nothing for a license the
        // customer brings, so the option carries no savings figure.
        instance(
            "i-0a1b2c3d4e5f70003",
            "m5.xlarge",
            4,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_BRING_YOUR_OWN,
            ApplicationInsights::Reporting,
            Some("sql-analytics"),
            LICENSE_EDITION_FREE,
            93.0,
            &[],
        ),
        // No CloudWatch Application Insights application watches this instance, so the
        // analysis has no license metrics at all.
        instance(
            "i-0a1b2c3d4e5f70004",
            "m5.large",
            2,
            LICENSE_EDITION_STANDARD,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::InvalidSetup,
            None,
            LICENSE_EDITION_STANDARD,
            93.0,
            &[("Owner", "TeamB")],
        ),
        // Watched, but the metric source is answering with an error.
        instance(
            "i-0a1b2c3d4e5f70005",
            "c5.2xlarge",
            8,
            LICENSE_EDITION_ENTERPRISE,
            LICENSE_MODEL_LICENSE_INCLUDED,
            ApplicationInsights::Failing,
            Some("sql-billing"),
            LICENSE_EDITION_ENTERPRISE,
            93.0,
            &[],
        ),
    ]
}
