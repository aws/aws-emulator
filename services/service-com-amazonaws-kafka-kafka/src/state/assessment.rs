// SPDX-License-Identifier: CC0-1.0

//! The assessments a cluster holds: the record `CreateAssessment` mints, the scan
//! it runs over the cluster the request names, the clock-owed statuses that scan
//! walks, and the projections its readers report.
//!
//! An assessment is a sub-resource of exactly one cluster — the model addresses it
//! as `/v1/clusters/{ClusterArn}/assessments/{AssessmentArn}` — so it is resolved
//! only through that cluster and disappears with it, the way a channel does.
//!
//! The FINDINGS are the point of the resource, and every one of them is computed
//! from this emulator's own state at the instant the scan runs: the cluster's
//! state, the broker type and Apache Kafka version it runs, the custom MSK
//! configuration it was given and the topics it holds. Each check reads state and
//! nothing else — no finding is minted for a condition this service cannot
//! observe — and each is classified with the model's own `FindingCategory` and
//! `FindingSeverity` values. `AssessmentFinding$FindingId` is "generated from
//! hash(category + resource + check_type)", so that is literally what
//! [`finding_id`] digests: the same resource with the same problem produces the
//! same id on every run.
//!
//! Its own file so the parent state module keeps to the cluster registry.

use super::*;

/// How long after `CreateAssessment` the scan it kicked off is COMPLETE.
///
/// CHOSEN, not documented: the real service scans a cluster asynchronously. It is
/// the span every clock-owed transition of this service takes
/// ([`CLUSTER_ACTIVATION_DELAY`]), so a caller that reads the assessment
/// immediately still observes `IN_PROGRESS` — and the refusal that goes with it —
/// while one that waits sees `COMPLETE`.
pub const ASSESSMENT_SCAN_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// How long after `DeleteAssessment` the assessment is gone from the registry,
/// chosen as [`ASSESSMENT_SCAN_DELAY`] is: the record stays resolvable in
/// `DELETING` until then, which is the status that call reports.
pub const ASSESSMENT_DELETION_DELAY: Duration = CLUSTER_ACTIVATION_DELAY;

/// The `AssessmentStatus` of an assessment whose scan is still running.
pub const ASSESSMENT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `AssessmentStatus` of an assessment whose scan has finished.
pub const ASSESSMENT_STATUS_COMPLETE: &str = "COMPLETE";

/// The `AssessmentStatus` of an assessment whose deletion has been accepted.
pub const ASSESSMENT_STATUS_DELETING: &str = "DELETING";

/// The one `AssessmentType` the model names.
pub const ASSESSMENT_TYPE_UPDATE_BROKER_TYPE: &str = "UPDATE_BROKER_TYPE";

/// How many assessments one cluster may hold at a time — the quota
/// `DeleteAssessment` "frees one slot against".
///
/// CHOSEN, not documented: it bounds what one cluster can hold in this emulator,
/// and is sized well above the handful of scans a caller runs while preparing one
/// migration.
pub const MAX_ASSESSMENTS_PER_CLUSTER: usize = 10;

/// How many findings one assessment STORES, past which it reports
/// `AssessmentSummary$FindingsCapped`: "True when the assessment detected more
/// findings than it stores."
///
/// CHOSEN, not documented: the model gives the cap no value, only the flag that
/// says it was reached. It is the largest page `ListAssessmentFindings` serves
/// ("Default 100, maximum 500" is its `MaxResults`), so a stored finding set is
/// always readable in one page or two.
pub const MAX_STORED_ASSESSMENT_FINDINGS: usize = 100;

/// The `FindingCategory` values, as the model's enum spells them.
pub const FINDING_CATEGORY_INFRASTRUCTURE: &str = "INFRASTRUCTURE";
/// See [`FINDING_CATEGORY_INFRASTRUCTURE`].
pub const FINDING_CATEGORY_BROKER_CONFIGURATION: &str = "BROKER_CONFIGURATION";
/// See [`FINDING_CATEGORY_INFRASTRUCTURE`].
pub const FINDING_CATEGORY_TOPIC_CONFIGURATION: &str = "TOPIC_CONFIGURATION";
/// See [`FINDING_CATEGORY_INFRASTRUCTURE`].
pub const FINDING_CATEGORY_CAPACITY: &str = "CAPACITY";
/// See [`FINDING_CATEGORY_INFRASTRUCTURE`].
pub const FINDING_CATEGORY_HEALTH: &str = "HEALTH";

/// The `FindingSeverity` values, as the model's enum spells them. An `ERROR`
/// finding is what `AssessmentSummary$HasBlockingFindings` counts: "ERROR findings
/// block the migration until resolved."
pub const FINDING_SEVERITY_ERROR: &str = "ERROR";
/// See [`FINDING_SEVERITY_ERROR`].
pub const FINDING_SEVERITY_WARNING: &str = "WARNING";
/// See [`FINDING_SEVERITY_ERROR`].
pub const FINDING_SEVERITY_INFO: &str = "INFO";

/// The replication factor Amazon MSK recommends for a cluster's topics, and so
/// the factor the topic check below reports a topic below: one replica in each of
/// the three Availability Zones an MSK cluster spreads its brokers over, which is
/// the default `CreateTopicRequest$ReplicationFactor` of a three-zone cluster.
pub const RECOMMENDED_REPLICATION_FACTOR: i32 = 3;

/// The documentation an assessment finding points a reader at for the broker types
/// the service offers: the Amazon MSK Developer Guide page the broker catalog
/// itself is sourced from ([`BROKER_INSTANCE_TYPE_CATALOG`]).
const BROKER_TYPES_DOCUMENTATION_URL: &str =
    "https://docs.aws.amazon.com/msk/latest/developerguide/msk-brokers.html";

/// One stored assessment of a cluster: what was asked for, when the scan ran, and
/// the findings it produced.
///
/// The findings are computed ONCE, by the scan the create kicked off, and then
/// reported from the record: "the same resource with the same problem always
/// produces the same findingId across assessment runs", and an assessment
/// describes the cluster as the scan found it rather than as it is now.
#[derive(Debug, Clone)]
pub struct Assessment {
    pub arn: String,
    /// The ARN of the cluster the assessment was run against — half of every
    /// lookup, since an assessment is addressed under its cluster.
    pub cluster_arn: String,
    /// The region the assessment lives in: the signing region of the call that
    /// created it, and the region segment of its ARN.
    pub region: String,
    /// An `AssessmentType` value, as the request named it.
    pub assessment_type: String,
    /// `AssessmentParameters$TargetInstanceType` as the request supplied it, or
    /// `None` when it named none — which is what makes the scan report a
    /// recommended instance type instead.
    pub target_instance_type: Option<String>,
    pub creation_time: SystemTime,
    /// The instant the clock owes this assessment its `IN_PROGRESS` ->
    /// `COMPLETE` transition. Stored, never slept on.
    pub completion_due: SystemTime,
    /// Set by `DeleteAssessment`: the instant the clock owes this assessment's
    /// removal from the registry. `None` for one nobody has deleted.
    pub removal_due: Option<SystemTime>,
    /// An `AssessmentStatus` value.
    pub status: String,
    /// Ordered so every reader renders a stable map.
    pub tags: BTreeMap<String, String>,
    /// The findings the scan stored, in the order it detected them, at most
    /// [`MAX_STORED_ASSESSMENT_FINDINGS`] of them.
    pub findings: Vec<AssessmentFinding>,
    /// How many findings the scan DETECTED, including the ones beyond the stored
    /// cap — which is what `AssessmentSummary$TotalFindings` reports.
    pub detected_findings: i32,
}

impl Assessment {
    /// Whether a reader at `now` still sees this assessment: one whose removal
    /// the clock already owes is gone, whether or not the record has been dropped
    /// yet.
    pub fn is_present(&self, now: SystemTime) -> bool {
        self.removal_due.is_none_or(|due| now < due)
    }

    /// Apply the transition to `COMPLETE` once `now` has reached
    /// [`Self::completion_due`]. Idempotent, and a no-op for every other status:
    /// only a transition the clock already owed is materialised here, and an
    /// assessment that is `DELETING` never goes back to `COMPLETE`.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == ASSESSMENT_STATUS_IN_PROGRESS && now >= self.completion_due {
            self.status = ASSESSMENT_STATUS_COMPLETE.to_string();
        }
    }

    /// Accept this assessment's deletion at `now`: move it to `DELETING` and
    /// record the instant the clock owes its removal.
    ///
    /// Idempotent — an assessment already `DELETING` keeps the removal instant it
    /// was given, so repeating the call cannot postpone the removal.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.status = ASSESSMENT_STATUS_DELETING.to_string();
        self.removal_due = Some(now + ASSESSMENT_DELETION_DELAY);
    }

    /// Whether this assessment's scan has finished, which is the state
    /// `ListAssessmentFindings` reports findings from: "returns the findings for a
    /// COMPLETED assessment".
    pub fn is_complete(&self) -> bool {
        self.status == ASSESSMENT_STATUS_COMPLETE
    }

    /// The key a cluster's assessments are listed by: the instant the assessment
    /// was created, latest first, with its own ARN breaking a tie so the order is
    /// TOTAL — the same key shape a cluster's operation history is ordered by
    /// ([`ClusterOperation::history_key`]), and the key the listing's page token
    /// carries.
    pub fn listing_key(&self) -> (std::cmp::Reverse<u128>, &str) {
        (
            std::cmp::Reverse(millis_since_epoch(self.creation_time)),
            self.arn.as_str(),
        )
    }

    /// The scan's findings grouped as `AssessmentSummary`: the counts over the
    /// findings this record STORES, the total the scan detected, whether that
    /// total was capped, and whether any stored finding blocks the migration.
    pub fn summary(&self) -> AssessmentSummary {
        let mut by_category: std::collections::HashMap<String, i32> =
            std::collections::HashMap::new();
        let mut by_severity: std::collections::HashMap<String, i32> =
            std::collections::HashMap::new();
        for finding in &self.findings {
            *by_category.entry(finding.category.clone()).or_default() += 1;
            *by_severity.entry(finding.severity.clone()).or_default() += 1;
        }
        AssessmentSummary {
            has_blocking_findings: Some(
                self.findings
                    .iter()
                    .any(|finding| finding.severity == FINDING_SEVERITY_ERROR),
            ),
            findings_capped: Some(
                self.detected_findings > i32::try_from(self.findings.len()).unwrap_or(i32::MAX),
            ),
            total_findings: Some(self.detected_findings),
            by_category: Some(by_category),
            by_severity: Some(by_severity),
        }
    }

    /// The assessment as `AssessmentInfo`, the shape `ListAssessments` pages.
    pub fn to_assessment_info(&self) -> AssessmentInfo {
        AssessmentInfo {
            assessment_arn: Some(self.arn.clone()),
            assessment_type: Some(self.assessment_type.clone()),
            cluster_arn: Some(self.cluster_arn.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            status: Some(self.status.clone()),
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }

    /// The same stored record as `DescribeAssessment` reports it: the identity and
    /// status the listing carries, plus the parameters the scan ran with and the
    /// summary it produced.
    ///
    /// `CompletionTime` is reported only once the scan has finished, since one
    /// still running has not completed; `StatusMessage` says what the status means
    /// for a scan that is not complete and nothing for one that is — there is
    /// nothing to explain about a finished scan.
    pub fn to_describe_output(&self) -> DescribeAssessmentOutput {
        DescribeAssessmentOutput {
            assessment_arn: Some(self.arn.clone()),
            assessment_parameters: Some(AssessmentParameters {
                target_instance_type: self.target_instance_type.clone(),
            }),
            assessment_type: Some(self.assessment_type.clone()),
            cluster_arn: Some(self.cluster_arn.clone()),
            completion_time: self
                .is_complete()
                .then(|| ::aws_smithy_types::DateTime::from(self.completion_due)),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            status: Some(self.status.clone()),
            status_message: self.status_message(),
            summary: self.is_complete().then(|| self.summary()),
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }

    /// What this assessment's status means, for a status that needs saying: a scan
    /// still running has no findings to report yet, and one being deleted is on
    /// its way out. A `COMPLETE` assessment reports none — its summary is the
    /// answer.
    fn status_message(&self) -> Option<String> {
        match self.status.as_str() {
            ASSESSMENT_STATUS_IN_PROGRESS => {
                Some("The assessment is scanning the cluster.".to_string())
            }
            ASSESSMENT_STATUS_DELETING => {
                Some("The assessment and its findings are being deleted.".to_string())
            }
            _ => None,
        }
    }
}

/// The `AssessmentFinding$FindingId` of a finding: "a deterministic content-based
/// identifier for this finding. Generated from hash(category + resource +
/// check_type)."
///
/// Exactly those three fields are digested, through this crate's one identifier
/// digest, so the same resource with the same problem produces the same id on
/// every run of the scan and two different problems of one resource produce two
/// ids.
fn finding_id(category: &str, resource: Option<&str>, check_type: &str) -> String {
    uuid_v4_shaped_from_digest(&stable_digest(&format!(
        "{category}:{}:{check_type}",
        resource.unwrap_or_default()
    )))
}

/// One finding, with the id the model's own rule derives for it.
fn finding(
    category: &str,
    severity: &str,
    check_type: &str,
    resource: Option<String>,
    message: String,
    remediation: Option<FindingRemediation>,
) -> AssessmentFinding {
    AssessmentFinding {
        category: category.to_string(),
        finding_id: finding_id(category, resource.as_deref(), check_type),
        message,
        remediation,
        resource,
        severity: severity.to_string(),
    }
}

/// The instant `at` in whole milliseconds since the Unix epoch — the granularity
/// a listing's page token can carry, so it is the granularity the listings here
/// order by.
fn millis_since_epoch(at: SystemTime) -> u128 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

impl ComAmazonawsKafkaKafkaState {
    /// Run an `UPDATE_BROKER_TYPE` scan of `cluster` and mint the assessment that
    /// holds its findings: `suffix` is the registry's monotonic counter, `now` the
    /// instant the scan was kicked off.
    ///
    /// Every finding is computed from stored state — the cluster record and the
    /// topics this registry holds for it — so an assessment of a cluster with
    /// nothing to report carries no findings at all rather than an invented one.
    pub fn scanned_assessment(
        &self,
        cluster: &Cluster,
        suffix: u64,
        now: SystemTime,
        input: CreateAssessmentInput,
    ) -> Assessment {
        let target_instance_type = input
            .assessment_parameters
            .and_then(|parameters| parameters.target_instance_type)
            .filter(|instance_type| !instance_type.is_empty());
        let detected = self.assessment_findings(cluster, target_instance_type.as_deref());
        let detected_findings = i32::try_from(detected.len()).unwrap_or(i32::MAX);
        let mut findings = detected;
        findings.truncate(MAX_STORED_ASSESSMENT_FINDINGS);
        // The registry's counter, as a 1-based ordinal, seeds the id: an
        // assessment's ARN is distinct from every other assessment's of the same
        // cluster, and stable for as long as the record is held.
        let ordinal = suffix + 1;
        let assessment_id = uuid_v4_shaped_from_digest(&stable_digest(&format!(
            "{}:{}:assessment:{ordinal}",
            ::engine::emulator_account_id(),
            cluster.arn
        )));
        // The cluster's own resource id, so an assessment ARN names the cluster it
        // belongs to the way every other MSK sub-resource ARN does.
        let cluster_id = cluster.arn.split('/').next_back().unwrap_or_default();
        Assessment {
            arn: ::engine::emulator_arn(
                "kafka",
                &cluster.region,
                &format!(
                    "assessment/{}/{cluster_id}/{assessment_id}",
                    cluster.name
                ),
            ),
            cluster_arn: cluster.arn.clone(),
            region: cluster.region.clone(),
            assessment_type: input.assessment_type,
            target_instance_type,
            creation_time: now,
            completion_due: now + ASSESSMENT_SCAN_DELAY,
            removal_due: None,
            status: ASSESSMENT_STATUS_IN_PROGRESS.to_string(),
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            findings,
            detected_findings,
        }
    }

    /// Every finding an `UPDATE_BROKER_TYPE` scan of `cluster` detects, in the
    /// order the checks run; `target_instance_type` is the broker type the scan
    /// was asked to assess against, or `None` when the request named none.
    ///
    /// The checks, each a function of stored state alone:
    ///
    /// * HEALTH — the cluster is not in a state an `UpdateBrokerType` is accepted
    ///   from ([`state_gate::cluster_accepts_change`]), which blocks the
    ///   migration until it settles.
    /// * INFRASTRUCTURE — the target broker type is not one the service offers
    ///   ([`is_catalogued_broker_instance_type`]), or it is one whose broker
    ///   category does not offer the Apache Kafka version the cluster runs
    ///   ([`create_kafka_version_violation`]).
    /// * CAPACITY — the request named no target type, so the scan reports the one
    ///   this cluster's size maps to ([`express_instance_type_for`]), which is
    ///   what `AssessmentParameters$TargetInstanceType` documents.
    /// * BROKER_CONFIGURATION — the cluster runs a custom MSK configuration,
    ///   whose properties the upgrade carries over.
    /// * TOPIC_CONFIGURATION — a topic of the cluster is replicated fewer than
    ///   [`RECOMMENDED_REPLICATION_FACTOR`] times.
    fn assessment_findings(
        &self,
        cluster: &Cluster,
        target_instance_type: Option<&str>,
    ) -> Vec<AssessmentFinding> {
        let mut findings = Vec::new();

        if !state_gate::cluster_accepts_change(&cluster.state) {
            findings.push(finding(
                FINDING_CATEGORY_HEALTH,
                FINDING_SEVERITY_ERROR,
                "cluster-state",
                None,
                format!(
                    "The cluster is in the {} state. {}",
                    cluster.state,
                    state_gate::not_in_accepted_state_message(
                        &state_gate::CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM
                    )
                ),
                None,
            ));
        }

        match target_instance_type {
            Some(target) if !is_catalogued_broker_instance_type(target) => {
                findings.push(finding(
                    FINDING_CATEGORY_INFRASTRUCTURE,
                    FINDING_SEVERITY_ERROR,
                    "target-instance-type",
                    None,
                    unsupported_instance_type_message(),
                    Some(FindingRemediation {
                        action: Some(
                            "Assess against a broker type Amazon MSK offers.".to_string(),
                        ),
                        documentation_url: Some(BROKER_TYPES_DOCUMENTATION_URL.to_string()),
                    }),
                ));
            }
            Some(target) => {
                if let Some(reason) =
                    create_kafka_version_violation(&cluster.kafka_version, target)
                {
                    findings.push(finding(
                        FINDING_CATEGORY_INFRASTRUCTURE,
                        FINDING_SEVERITY_ERROR,
                        "kafka-version",
                        None,
                        reason,
                        Some(FindingRemediation {
                            action: Some(
                                "Update the cluster's Apache Kafka version to one the target \
                                 broker category offers, then assess the cluster again."
                                    .to_string(),
                            ),
                            documentation_url: Some(BROKER_TYPES_DOCUMENTATION_URL.to_string()),
                        }),
                    ));
                }
            }
            None => {
                if let Some(recommended) =
                    express_instance_type_for(&cluster.broker_node_group_info.instance_type)
                {
                    findings.push(finding(
                        FINDING_CATEGORY_CAPACITY,
                        FINDING_SEVERITY_INFO,
                        "recommended-instance-type",
                        None,
                        format!(
                            "The cluster's brokers run {}. Amazon MSK recommends {recommended} \
                             for them, which is the Express broker type of that size.",
                            cluster.broker_node_group_info.instance_type
                        ),
                        Some(FindingRemediation {
                            action: Some(format!(
                                "Assess the cluster against {recommended}, then update its \
                                 broker type to it."
                            )),
                            documentation_url: Some(BROKER_TYPES_DOCUMENTATION_URL.to_string()),
                        }),
                    ));
                }
            }
        }

        if let Some(configuration) = cluster.configuration_info.as_ref() {
            findings.push(finding(
                FINDING_CATEGORY_BROKER_CONFIGURATION,
                FINDING_SEVERITY_INFO,
                "custom-configuration",
                Some(configuration.arn.clone()),
                format!(
                    "The cluster's brokers run revision {} of this MSK configuration. The \
                     upgrade carries its properties over, so review them against the target \
                     broker type first.",
                    configuration.revision
                ),
                None,
            ));
        }

        for topic in self
            .topics_in(&cluster.arn)
            .filter(|topic| topic.replication_factor < RECOMMENDED_REPLICATION_FACTOR)
        {
            findings.push(finding(
                FINDING_CATEGORY_TOPIC_CONFIGURATION,
                FINDING_SEVERITY_WARNING,
                "topic-replication-factor",
                Some(topic.name.clone()),
                format!(
                    "The topic is replicated {} time(s). Amazon MSK recommends a replication \
                     factor of {RECOMMENDED_REPLICATION_FACTOR}, so that every partition of it \
                     keeps a replica while its brokers move to another broker type.",
                    topic.replication_factor
                ),
                Some(FindingRemediation {
                    action: Some(format!(
                        "Raise the topic's replication factor to \
                         {RECOMMENDED_REPLICATION_FACTOR}."
                    )),
                    documentation_url: None,
                }),
            ));
        }

        findings
    }

    /// How many assessments the cluster `cluster_arn` holds at `now`, which is
    /// what the per-cluster quota is enforced against.
    pub fn assessment_count_in_cluster(&self, cluster_arn: &str, now: SystemTime) -> usize {
        self.assessments
            .values()
            .filter(|assessment| assessment.cluster_arn == cluster_arn && assessment.is_present(now))
            .count()
    }

    /// Whether the cluster `cluster_arn` has a scan still running at `now` — one
    /// assessment of a cluster at a time, which is what makes a second
    /// `CreateAssessment` a conflict rather than a second record.
    pub fn assessment_in_progress_in_cluster(&self, cluster_arn: &str, now: SystemTime) -> bool {
        self.assessments.values().any(|assessment| {
            assessment.cluster_arn == cluster_arn
                && assessment.is_present(now)
                && assessment.status == ASSESSMENT_STATUS_IN_PROGRESS
                && now < assessment.completion_due
        })
    }

    /// Register `assessment` under its own ARN. The caller has already admitted
    /// it against the per-cluster quota, which is what bounds this map together
    /// with the cluster registry it hangs off.
    pub fn register_assessment(&mut self, assessment: Assessment) -> String {
        let arn = assessment.arn.clone();
        self.next_assessment_suffix = self.next_assessment_suffix.saturating_add(1);
        self.assessments.insert(arn.clone(), assessment);
        arn
    }

    /// The assessment `assessment_arn` names under `cluster_arn`, as an endpoint
    /// for `region` sees it at `now`, with the transitions the clock already owed
    /// it materialised: one whose removal is due is gone (so this answers `None`),
    /// and one whose scan has finished is `COMPLETE`.
    ///
    /// The ONE lookup every assessment reader answers from, so a listing and a
    /// describe of one assessment cannot disagree about its status.
    pub fn settled_assessment_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        assessment_arn: &str,
        now: SystemTime,
    ) -> Option<&Assessment> {
        let assessment = self.assessments.get(assessment_arn)?;
        if assessment.region != region || assessment.cluster_arn != cluster_arn {
            return None;
        }
        if !assessment.is_present(now) {
            self.assessments.remove(assessment_arn);
            return None;
        }
        let assessment = self.assessments.get_mut(assessment_arn)?;
        assessment.settle(now);
        Some(assessment)
    }

    /// The assessments `cluster_arn` holds, as an endpoint for `region` sees them
    /// at `now`, latest first ([`Assessment::listing_key`]) — the listing
    /// counterpart of [`Self::settled_assessment_in`].
    ///
    /// Reporting only, and the writes are the ones the clock already owed: the
    /// removals this cluster's assessments were due are materialised first, so one
    /// every other reader calls gone is not listed, and each remaining scan's
    /// completion is applied if it was due.
    pub fn settled_assessments_in(
        &mut self,
        region: &str,
        cluster_arn: &str,
        now: SystemTime,
    ) -> Vec<&Assessment> {
        let arns: Vec<String> = self
            .assessments
            .values()
            .filter(|assessment| assessment.cluster_arn == cluster_arn)
            .map(|assessment| assessment.arn.clone())
            .collect();
        for arn in &arns {
            match self.assessments.get(arn) {
                Some(assessment) if !assessment.is_present(now) => {
                    self.assessments.remove(arn);
                }
                _ => {
                    if let Some(assessment) = self.assessments.get_mut(arn) {
                        assessment.settle(now);
                    }
                }
            }
        }
        let mut assessments: Vec<&Assessment> = self
            .assessments
            .values()
            .filter(|assessment| {
                assessment.cluster_arn == cluster_arn && assessment.region == region
            })
            .collect();
        assessments.sort_by(|left, right| left.listing_key().cmp(&right.listing_key()));
        assessments
    }

    /// Accept the deletion of the assessment `assessment_arn` names under
    /// `cluster_arn`, handing back the record as the caller must now see it:
    /// `DELETING`, with the instant the clock owes its removal set.
    ///
    /// Answers `None` when this region holds no such assessment under that
    /// cluster — including one whose removal the clock already owed, which the
    /// settling lookup materialises first, so an assessment is never deleted
    /// twice. Idempotent through [`Assessment::begin_deletion`].
    pub fn begin_assessment_deletion(
        &mut self,
        region: &str,
        cluster_arn: &str,
        assessment_arn: &str,
        now: SystemTime,
    ) -> Option<&Assessment> {
        self.settled_assessment_in(region, cluster_arn, assessment_arn, now)?;
        let assessment = self.assessments.get_mut(assessment_arn)?;
        assessment.begin_deletion(now);
        self.assessments.get(assessment_arn)
    }

    /// Drop every assessment of the cluster `cluster_arn`, which is what makes an
    /// assessment disappear with the cluster it was run against.
    pub fn drop_cluster_assessments(&mut self, cluster_arn: &str) {
        self.assessments
            .retain(|_, assessment| assessment.cluster_arn != cluster_arn);
    }
}
