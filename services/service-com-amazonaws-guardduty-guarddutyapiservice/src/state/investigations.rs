// SPDX-License-Identifier: CC0-1.0

//! AI-analyst investigations: the record one investigation is held as, the
//! Regions the capability is offered in, and the ordering a listing reports them
//! in.

use super::*;

/// `InvestigationStatus`, the status a created investigation starts in: the
/// analysis is under way. One of `RUNNING | COMPLETED | FAILED`
/// (`smithy_query(shape="InvestigationStatus")`).
pub const INVESTIGATION_RUNNING: &str = "RUNNING";

/// The `InvestigationStatus` an investigation reaches when its analysis pass has
/// run: the terminal status of an accepted investigation, whose conclusions the
/// record then carries ([`InvestigationAnalysis`]). See [`INVESTIGATION_RUNNING`]
/// for the enum.
pub const INVESTIGATION_COMPLETED: &str = "COMPLETED";

/// How long a created investigation stays [`INVESTIGATION_RUNNING`] before the
/// clock owes it a terminal status.
///
/// GuardDuty documents no duration for an investigation, so this emulator CHOOSES
/// one. It is deliberately non-zero: a caller that reads the investigation back
/// immediately after creating it still observes the transient `RUNNING` status.
pub const INVESTIGATION_TRANSITION_MILLIS: i64 = 500;

/// The window the per-day investigation limit is counted over: "you can initiate
/// up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATION_DAY_MILLIS: i64 = DAY_MILLIS;

/// How many investigations an account may initiate per day: "During the preview,
/// you can initiate up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_DAY: usize = 10;

/// How many investigations an account may initiate in total: "with a total limit
/// of 100 investigations per account"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_ACCOUNT: usize = 100;

/// The Regions GuardDuty Investigation is available in, as `CreateInvestigation`
/// and `GetInvestigation` both list them: "US East (N. Virginia), US East (Ohio),
/// US West (Oregon), Canada (Central), Europe (Frankfurt), Europe (Ireland),
/// Europe (London), Europe (Paris), Europe (Stockholm), and Asia Pacific (Tokyo)"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`), in that order.
pub const INVESTIGATION_REGIONS: [&str; 10] = [
    "us-east-1",
    "us-east-2",
    "us-west-2",
    "ca-central-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "ap-northeast-1",
];

/// Is GuardDuty Investigation available through `region`'s endpoint? Decided from
/// the documented [`INVESTIGATION_REGIONS`] catalogue, not from what the store
/// happens to hold.
pub fn investigations_available_in(region: &str) -> bool {
    INVESTIGATION_REGIONS.contains(&region)
}

/// The message a request to a Region the feature is not available in is refused
/// with. It does not name the caller's Region: the endpoint already carries it.
pub const INVESTIGATION_REGION_MESSAGE: &str =
    "The request is rejected because GuardDuty Investigation is not available in the current Region.";

/// The message `GetInvestigation` answers an `InvestigationId` this detector does
/// not hold with: the Investigation family's own not-found sentence, a short fixed
/// text naming neither the operation, nor the member that carried the id, nor the
/// id itself — GuardDuty does not echo the caller's id back.
pub const INVESTIGATION_NOT_FOUND_MESSAGE: &str = "Resource not found.";

/// The message `ListInvestigationRecords` refuses a `NextToken` handed to an
/// investigation whose pass has not ended with: "if you specify this parameter for an
/// investigation whose status isn't COMPLETED, the operation returns a
/// BadRequestException"
/// (`smithy_query(query="doc",shape="ListInvestigationRecordsRequest",member="NextToken")`).
///
/// Entirely FIXED text in the Investigation family's own wording: it states what the
/// request asked for that cannot be served, and quotes neither the token nor the
/// investigation back.
pub const INVESTIGATION_RECORDS_TOKEN_MESSAGE: &str =
    "The request is rejected because nextToken cannot be used for an investigation whose status is not COMPLETED.";

/// The message `CreateInvestigation` refuses a detector that does not have the
/// AI_ANALYST feature enabled with — "to use this operation, the AI_ANALYST feature
/// must be enabled on your detector"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
///
/// Entirely FIXED text, naming no account and no detector: it states the
/// prerequisite and how to meet it, and nothing of the request is interpolated.
pub const INVESTIGATION_FEATURE_DENIED_MESSAGE: &str =
    "The request is rejected because the AI Analyst feature is not enabled for the delegated administrator account. Enable the AI_ANALYST feature on your delegated administrator's detector to use the Investigation API.";

/// The message a request past the documented per-day investigation limit is
/// refused with.
pub fn investigation_daily_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_DAY} investigations per day."
    )
}

/// The message a request past the documented total investigation limit is refused
/// with.
pub fn investigation_total_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_ACCOUNT} investigations."
    )
}

/// `CloudProvider`'s only member — "The cloud provider. Currently, only `AWS` is
/// supported" (`smithy_query(query="doc",shape="CloudDetails",member="Provider")`).
pub const CLOUD_PROVIDER_AWS: &str = "AWS";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::CLOUD_PROVIDER_VALUES,
    CLOUD_PROVIDER_AWS
));

/// The cloud environment ONE investigation covered, held on that investigation's own
/// record: the three `CloudDetails` members — the provider, the Region and the
/// account of the environment the analysis pass ran over
/// (`smithy_query(shape="CloudDetails")`).
///
/// It is resolved exactly once, when the pass ENDS
/// ([`InvestigationRecord::settle`]), and read back from the record afterwards, so
/// the environment a reader reports is the one THIS investigation covered rather
/// than anything about the request the read arrives on: an investigation that has
/// not finished its pass has resolved no environment and holds none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationCloud {
    /// `CloudDetails$Provider` — see [`CLOUD_PROVIDER_AWS`].
    pub provider: String,
    /// `CloudDetails$Region`: the Region whose resources the pass covered.
    pub region: String,
    /// `CloudDetails$Account`: the account whose resources the pass covered.
    pub account: String,
}

/// `InvestigationType`: "An investigation of a specific GuardDuty finding"
/// (`smithy_query(query="doc",shape="InvestigationScope",member="InvestigationType")`).
/// The enum is `FINDING | ACCOUNT | ORGANIZATION`.
pub const INVESTIGATION_TYPE_FINDING: &str = "FINDING";

/// `InvestigationType`: "An investigation of the threat posture of an Amazon Web
/// Services account". See [`INVESTIGATION_TYPE_FINDING`] for the enum.
pub const INVESTIGATION_TYPE_ACCOUNT: &str = "ACCOUNT";

/// `InvestigationType`: "An investigation of your organization's threat posture".
pub const INVESTIGATION_TYPE_ORGANIZATION: &str = "ORGANIZATION";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_TYPE_VALUES,
    INVESTIGATION_TYPE_FINDING
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_TYPE_VALUES,
    INVESTIGATION_TYPE_ACCOUNT
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_TYPE_VALUES,
    INVESTIGATION_TYPE_ORGANIZATION
));

/// How far back an account-wide or organization-wide pass queries data: the span
/// `QueryTimeWindow` reports for it, ending at the instant the pass ran.
///
/// GuardDuty documents no span for the window, so this emulator CHOOSES one. It is
/// the bound the pass actually applies to the findings it reads
/// ([`InvestigationRecord::settle`]), which is what makes the reported window a
/// fact about the pass rather than a decoration.
pub const INVESTIGATION_QUERY_WINDOW_MILLIS: i64 = 30 * DAY_MILLIS;

/// `SummaryFormat`: the structured summary, the format a request asks for to be
/// served `SummaryData` instead of the text `Summary`. The enum is `STRING |
/// STRUCTURED` (`smithy_query(shape="SummaryFormat")`).
pub const SUMMARY_FORMAT_STRUCTURED: &str = "STRUCTURED";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::SUMMARY_FORMAT_VALUES,
    SUMMARY_FORMAT_STRUCTURED
));

/// What ONE investigation analysed, held on that investigation's own record: the
/// `InvestigationScope` members — the kind of subject, the subject itself, and the
/// window an account-wide or organization-wide pass queried
/// (`smithy_query(shape="InvestigationScope")`).
///
/// "Only the identifier that matches `InvestigationType` is set"
/// (`smithy_query(query="doc",shape="InvestigationScope")`), so exactly one of the
/// three identifiers below carries a value, and the window is set for the two types
/// whose pass queries a span of data rather than one named finding.
///
/// Resolved exactly once, when the pass ENDS ([`InvestigationRecord::settle`]), from
/// the prompt the investigation was initiated with and the membership the detector
/// held at that instant — so what a reader reports is what THIS pass covered, never
/// a function of the request the read arrives on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationScopeRecord {
    /// `InvestigationScope$InvestigationType` — see [`INVESTIGATION_TYPE_FINDING`].
    pub investigation_type: String,
    /// `InvestigationScope$FindingId`, set only for [`INVESTIGATION_TYPE_FINDING`].
    pub finding_id: Option<String>,
    /// `InvestigationScope$AccountId`, set only for [`INVESTIGATION_TYPE_ACCOUNT`].
    pub account_id: Option<String>,
    /// `InvestigationScope$AdminAccountId`, set only for
    /// [`INVESTIGATION_TYPE_ORGANIZATION`].
    pub admin_account_id: Option<String>,
    /// `QueryTimeWindow`, in epoch milliseconds: the span the pass queried, set for
    /// the two account-wide types. See [`INVESTIGATION_QUERY_WINDOW_MILLIS`].
    pub query_window_millis: Option<(i64, i64)>,
    /// The MEMBER accounts the detector administered when the pass ran, beside the
    /// administrator's own account. Not a member of `InvestigationScope`: it is the
    /// organization as the pass read it, which is what an organization-wide
    /// structured summary counts its accounts from
    /// ([`investigation_summary_data`]).
    pub organization_accounts: usize,
}

impl InvestigationScopeRecord {
    /// The scope of a pass that analysed the ONE finding `finding_id` names.
    fn of_finding(finding_id: String, organization_accounts: usize) -> Self {
        Self {
            investigation_type: INVESTIGATION_TYPE_FINDING.to_string(),
            finding_id: Some(finding_id),
            account_id: None,
            admin_account_id: None,
            query_window_millis: None,
            organization_accounts,
        }
    }

    /// The scope of a pass that swept ONE account's findings over `window`.
    fn of_account(account_id: String, window: (i64, i64)) -> Self {
        Self {
            investigation_type: INVESTIGATION_TYPE_ACCOUNT.to_string(),
            finding_id: None,
            account_id: Some(account_id),
            admin_account_id: None,
            query_window_millis: Some(window),
            organization_accounts: 0,
        }
    }

    /// The scope of a pass that swept the organization `admin_account_id`
    /// administers over `window`.
    fn of_organization(
        admin_account_id: String,
        window: (i64, i64),
        organization_accounts: usize,
    ) -> Self {
        Self {
            investigation_type: INVESTIGATION_TYPE_ORGANIZATION.to_string(),
            finding_id: None,
            account_id: None,
            admin_account_id: Some(admin_account_id),
            query_window_millis: Some(window),
            organization_accounts,
        }
    }

    /// The modelled `InvestigationScope` a reader reports for this scope.
    pub fn details(&self) -> crate::ops::types::InvestigationScope {
        crate::ops::types::InvestigationScope {
            investigation_type: Some(self.investigation_type.clone()),
            finding_id: self.finding_id.clone(),
            account_id: self.account_id.clone(),
            admin_account_id: self.admin_account_id.clone(),
            query_time_window: self.query_window_millis.map(|(start, end)| {
                crate::ops::types::QueryTimeWindow {
                    start_time: Some(::aws_smithy_types::DateTime::from_millis(start)),
                    end_time: Some(::aws_smithy_types::DateTime::from_millis(end)),
                }
            }),
        }
    }
}

/// One stored investigation. Timestamps are epoch milliseconds, the single source
/// for the instants the wire carries.
#[derive(Debug, Clone)]
pub struct InvestigationRecord {
    pub id: String,
    /// `TriggerPrompt`: what the caller asked to be investigated, stored verbatim.
    pub trigger_prompt: String,
    /// `TriggeredBy`: "The account that initiated the investigation"
    /// (`smithy_query(shape="Investigation")`).
    pub triggered_by: String,
    /// The Region the investigation was initiated through.
    pub region: String,
    /// The `ClientToken` the create carried, so a replay returns this same
    /// investigation instead of initiating a second one.
    pub client_token: Option<String>,
    /// `InvestigationStatus`.
    pub status: String,
    pub started_at_millis: i64,
    /// When the clock owes this investigation its terminal status. See
    /// [`INVESTIGATION_TRANSITION_MILLIS`].
    pub terminal_due_millis: i64,
    /// When it reached a terminal status, once it has.
    pub ended_at_millis: Option<i64>,
    /// What the analysis pass concluded, once it has run. See
    /// [`InvestigationAnalysis`], and [`Self::settle`] for when it is computed.
    pub analysis: Option<InvestigationAnalysis>,
    /// `Cloud`: the environment this investigation's pass covered, once the pass has
    /// ended and resolved it. See [`InvestigationCloud`].
    pub cloud: Option<InvestigationCloud>,
    /// `Scope`: what the pass analysed, once it has ended and resolved it. See
    /// [`InvestigationScopeRecord`].
    pub scope: Option<InvestigationScopeRecord>,
}

impl InvestigationRecord {
    /// An investigation initiated at `now`, still [`INVESTIGATION_RUNNING`].
    pub fn started(
        id: String,
        trigger_prompt: String,
        triggered_by: String,
        region: String,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id,
            trigger_prompt,
            triggered_by,
            region,
            client_token,
            status: INVESTIGATION_RUNNING.to_string(),
            started_at_millis: now,
            terminal_due_millis: now.saturating_add(INVESTIGATION_TRANSITION_MILLIS),
            ended_at_millis: None,
            // The pass has not run yet, so it has reached no conclusions.
            analysis: None,
            // The pass has not run yet, so it has resolved no environment: a reader
            // reports no `Cloud` member for an investigation still
            // [`INVESTIGATION_RUNNING`].
            cloud: None,
            // Nor has it resolved what it covers: `Scope` is "returned only when the
            // investigation status is COMPLETED"
            // (`smithy_query(query="doc",shape="Investigation",member="Scope")`).
            scope: None,
        }
    }

    /// The `Cloud` member a reader reports for this investigation: the environment
    /// stored on the record, and NOTHING while the record has resolved none.
    pub fn cloud_details(&self) -> Option<crate::ops::types::CloudDetails> {
        self.cloud
            .as_ref()
            .map(|cloud| crate::ops::types::CloudDetails {
                provider: cloud.provider.clone(),
                region: cloud.region.clone(),
                account: cloud.account.clone(),
            })
    }

    /// The `Metadata` member a reader reports for this investigation: the product
    /// and engine version that produced its conclusions, and NOTHING while no pass
    /// has produced any.
    pub fn metadata(&self) -> Option<crate::ops::types::InvestigationMetadata> {
        self.analysis
            .as_ref()
            .map(|_| InvestigationAnalysis::metadata())
    }

    /// The `Scope` member a reader reports for this investigation: what the pass
    /// resolved it covered, and NOTHING while it has resolved none.
    pub fn scope_details(&self) -> Option<crate::ops::types::InvestigationScope> {
        self.scope.as_ref().map(InvestigationScopeRecord::details)
    }

    /// The `InvestigationType` of this investigation — "GuardDuty sets this field
    /// when the investigation completes"
    /// (`smithy_query(shape="InvestigationSummary",member="InvestigationType")`) —
    /// and `None` for one whose pass has not ended.
    pub fn investigation_type(&self) -> Option<String> {
        self.scope
            .as_ref()
            .map(|scope| scope.investigation_type.clone())
    }

    /// The finding this investigation analysed, for the one type that analyses a
    /// single finding; `None` for every other scope, and for a pass that has not
    /// ended.
    pub fn scope_finding_id(&self) -> Option<String> {
        self.scope.as_ref().and_then(|scope| scope.finding_id.clone())
    }

    /// The administrator account of the organization this investigation analysed,
    /// for the one type that analyses an organization; `None` otherwise.
    pub fn scope_admin_account_id(&self) -> Option<String> {
        self.scope
            .as_ref()
            .and_then(|scope| scope.admin_account_id.clone())
    }

    /// Apply the status change the CLOCK already made due, if any, and with it the
    /// conclusions of the analysis pass that change ends.
    ///
    /// The move out of `RUNNING` was owed at [`Self::terminal_due_millis`] whether
    /// or not anybody called, which is why a reporting operation may apply it. It
    /// is applied exactly once: a terminal investigation never moves again.
    ///
    /// `evidence` is the findings the detector holds ([`FindingEvidence`]); the pass
    /// reads the ones that existed when it ENDED, so the assessment it stores is the
    /// one the service owed at that instant and does not depend on when a caller
    /// first asked for it. `member_accounts` is the organization the detector
    /// administers at that instant, which is what decides whether an account-wide
    /// prompt covers one account or the whole organization.
    pub fn settle(&mut self, now: i64, evidence: &[FindingEvidence], member_accounts: usize) {
        if self.status != INVESTIGATION_RUNNING || now < self.terminal_due_millis {
            return;
        }
        // Everything the account had produced by the instant the pass ran. A finding
        // produced after it is no part of what the pass could read.
        let existing: Vec<FindingEvidence> = evidence
            .iter()
            .filter(|finding| finding.created_at_millis <= self.terminal_due_millis)
            .cloned()
            .collect();
        // What the prompt asked about decides BOTH the scope the pass covered and the
        // evidence it read, so the two cannot disagree: a prompt that names findings
        // the account holds is an investigation OF those findings, and one that names
        // none sweeps the subject's own findings over the window it queried.
        let named = named_evidence(&self.trigger_prompt, &existing);
        let (scope, covered) = match worst_of(&named) {
            Some(worst) => (
                InvestigationScopeRecord::of_finding(worst.id.clone(), member_accounts),
                named,
            ),
            None => {
                let window = (
                    self.terminal_due_millis
                        .saturating_sub(INVESTIGATION_QUERY_WINDOW_MILLIS),
                    self.terminal_due_millis,
                );
                let swept: Vec<FindingEvidence> = existing
                    .into_iter()
                    .filter(|finding| finding.created_at_millis >= window.0)
                    .collect();
                let scope = match member_accounts {
                    // A detector that administers no member account has no
                    // organization to assess, so the subject is its own account.
                    0 => InvestigationScopeRecord::of_account(self.triggered_by.clone(), window),
                    accounts => InvestigationScopeRecord::of_organization(
                        self.triggered_by.clone(),
                        window,
                        accounts,
                    ),
                };
                (scope, swept)
            }
        };
        self.analysis = Some(InvestigationAnalysis::of_evidence(
            &covered,
            &self.trigger_prompt,
            &self.triggered_by,
            &self.region,
        ));
        self.scope = Some(scope);
        self.status = INVESTIGATION_COMPLETED.to_string();
        self.ended_at_millis = Some(self.terminal_due_millis);
        // The pass covered ONE environment — the account the investigation was
        // initiated for, through the Region its endpoint served, both persisted on
        // this record when the create minted it — so that environment is resolved
        // the instant the pass ends. Storing it here is what lets every later read
        // report the same provider/Region/account for this investigation, whatever
        // endpoint or caller the read arrives on.
        self.cloud = Some(InvestigationCloud {
            provider: CLOUD_PROVIDER_AWS.to_string(),
            region: self.region.clone(),
            account: self.triggered_by.clone(),
        });
    }
}

/// `OrderBy`, the order a sort runs in: the enum is `ASC | DESC`
/// (`smithy_query(shape="OrderBy")`), the two values the generated
/// [`crate::ops::types::shared_types::ORDER_BY_VALUES`] list carries. Every sort
/// in this service reads the member through these two constants, so none of them
/// can spell the order differently.
pub const ORDER_BY_ASCENDING: &str = "ASC";

/// The other `OrderBy` member — see [`ORDER_BY_ASCENDING`].
pub const ORDER_BY_DESCENDING: &str = "DESC";

/// The attributes a `ListInvestigations` sort may name: the
/// `InvestigationSortField` members, which the model lists as `START_TIME |
/// END_TIME | STATUS | RISK_LEVEL | CONFIDENCE`
/// (`smithy_query(shape="InvestigationSortField")`) and the generated enum carries
/// as [`crate::ops::types::shared_types::INVESTIGATION_SORT_FIELD_VALUES`]. Read
/// from the generated list so the two can never drift.
pub fn investigation_sort_fields() -> &'static [&'static str] {
    crate::ops::types::shared_types::INVESTIGATION_SORT_FIELD_VALUES
}

/// The attribute a `ListInvestigations` sort orders by when the request names
/// none: when the investigation started, which with [`DESCENDING_BY_DEFAULT`] puts
/// the newest investigation first — the order the other reporting operations of
/// this service answer an unordered request in.
pub const INVESTIGATION_DEFAULT_SORT_FIELD: &str = "START_TIME";

/// Resolve one investigation attribute by its `InvestigationSortField` name, or
/// `None` when that is not a member of the enum.
///
/// `RISK_LEVEL` and `CONFIDENCE` are the analysis's conclusions, so they resolve to
/// the RANK of the stored member on its own scale ([`risk_level_rank`],
/// [`confidence_rank`]) rather than to its spelling: ordering by a graded enum is
/// ordering by the grade, and an investigation whose pass has reached no conclusion
/// ranks below every one that has.
pub fn investigation_attribute(investigation: &InvestigationRecord, field: &str) -> Option<Attribute> {
    let analysis = investigation.analysis.as_ref();
    let value = match field {
        "START_TIME" => Attribute::Number(investigation.started_at_millis as f64),
        "END_TIME" => Attribute::Number(investigation.ended_at_millis.unwrap_or(0) as f64),
        "STATUS" => Attribute::Text(investigation.status.clone()),
        "RISK_LEVEL" => Attribute::Number(
            analysis.map_or(UNRANKED, |analysis| risk_level_rank(&analysis.risk_level)),
        ),
        "CONFIDENCE" => Attribute::Number(
            analysis.map_or(UNRANKED, |analysis| confidence_rank(&analysis.confidence)),
        ),
        _ => return None,
    };
    Some(value)
}

/// The criterion keys a `ListInvestigations` filter may name: the
/// `InvestigationFilterCriterionKey` members, which the model lists as `STATUS |
/// RISK_LEVEL | CONFIDENCE | INVESTIGATION_TYPE | FINDING_ID`
/// (`smithy_query(shape="InvestigationFilterCriterionKey")`) and the generated enum
/// carries as
/// [`crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES`].
/// Read from the generated list so the two can never drift.
pub fn investigation_filter_criterion_keys() -> &'static [&'static str] {
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES
}

/// `InvestigationFilterCriterionKey`: the investigation's own `Status`.
pub const INVESTIGATION_KEY_STATUS: &str = "STATUS";

/// `InvestigationFilterCriterionKey`: the assessed `RiskLevel`, which an
/// investigation has only once its pass has reached a conclusion. See
/// [`INVESTIGATION_KEY_STATUS`] for the enum.
pub const INVESTIGATION_KEY_RISK_LEVEL: &str = "RISK_LEVEL";

/// `InvestigationFilterCriterionKey`: the assessment's `Confidence`.
pub const INVESTIGATION_KEY_CONFIDENCE: &str = "CONFIDENCE";

/// `InvestigationFilterCriterionKey`: the `InvestigationType` of the scope the pass
/// resolved.
pub const INVESTIGATION_KEY_INVESTIGATION_TYPE: &str = "INVESTIGATION_TYPE";

/// `InvestigationFilterCriterionKey`: the finding a `FINDING` investigation analysed.
pub const INVESTIGATION_KEY_FINDING_ID: &str = "FINDING_ID";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES,
    INVESTIGATION_KEY_STATUS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES,
    INVESTIGATION_KEY_RISK_LEVEL
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES,
    INVESTIGATION_KEY_CONFIDENCE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES,
    INVESTIGATION_KEY_INVESTIGATION_TYPE
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::INVESTIGATION_FILTER_CRITERION_KEY_VALUES,
    INVESTIGATION_KEY_FINDING_ID
));

/// The value one investigation holds for a filter criterion key, or `None` where it
/// holds none — which is every key but [`INVESTIGATION_KEY_STATUS`] for an
/// investigation whose pass has not ended, and [`INVESTIGATION_KEY_FINDING_ID`] for
/// every scope that analysed no single finding.
///
/// Each value is the one the READS report for that member, so a filtered listing and
/// an unfiltered one cannot disagree about an investigation's own attributes.
pub fn investigation_criterion_value(
    investigation: &InvestigationRecord,
    key: &str,
) -> Option<String> {
    let analysis = investigation.analysis.as_ref();
    match key {
        INVESTIGATION_KEY_STATUS => Some(investigation.status.clone()),
        INVESTIGATION_KEY_RISK_LEVEL => analysis.map(|analysis| analysis.risk_level.clone()),
        INVESTIGATION_KEY_CONFIDENCE => analysis.map(|analysis| analysis.confidence.clone()),
        INVESTIGATION_KEY_INVESTIGATION_TYPE => investigation.investigation_type(),
        INVESTIGATION_KEY_FINDING_ID => investigation.scope_finding_id(),
        _ => None,
    }
}

/// Refuse a `FilterCriteria` naming a criterion key the model does not carry, with
/// the message the refusal carries.
///
/// Ignoring such a key would report investigations the caller asked to exclude, which
/// is why the coverage listing refuses an unknown criterion key too
/// ([`crate::state::coverage::requested_criteria`]); the sentence follows the one
/// every criteria-refusing report of this service answers with.
pub fn validate_investigation_criteria(
    criteria: Option<&crate::ops::types::InvestigationFilterCriteria>,
) -> Result<(), String> {
    for key in criteria
        .and_then(|criteria| criteria.criterion.as_ref())
        .into_iter()
        .flatten()
        .map(|(key, _)| key)
    {
        if !investigation_filter_criterion_keys().contains(&key.as_str()) {
            return Err(format!(
                "filterCriteria: '{key}' is not an investigation criterion key. Supported keys are {}.",
                investigation_filter_criterion_keys().join(", ")
            ));
        }
    }
    Ok(())
}

/// Does `investigation` satisfy EVERY entry of `criteria`? An absent or empty
/// criteria structure matches every investigation.
///
/// One entry's `Equals` admits an investigation whose value for the key is one of the
/// values it lists, so an investigation that holds no such value cannot satisfy it;
/// `NotEquals` excludes an investigation whose value is one of its values, so one
/// holding no such value is not excluded; and a condition that lists no value at all
/// restricts nothing. That is the rule the coverage listing's own criteria follow
/// ([`crate::state::coverage::Criterion`]), so two filtered reports of this service
/// cannot read one condition differently.
pub fn investigation_matches(
    investigation: &InvestigationRecord,
    criteria: Option<&crate::ops::types::InvestigationFilterCriteria>,
) -> bool {
    criteria
        .and_then(|criteria| criteria.criterion.as_ref())
        .into_iter()
        .flatten()
        .all(|(key, condition)| {
            let value = investigation_criterion_value(investigation, key);
            let equals = condition.equals.as_deref().unwrap_or_default();
            let not_equals = condition.not_equals.as_deref().unwrap_or_default();
            let admitted = equals.is_empty()
                || value
                    .as_deref()
                    .is_some_and(|value| equals.iter().any(|wanted| wanted == value));
            let not_excluded = value
                .as_deref()
                .is_none_or(|value| !not_equals.iter().any(|unwanted| unwanted == value));
            admitted && not_excluded
        })
}

/// Order `investigations` for `ListInvestigations`.
///
/// With no `SortCriteria` the newest investigation comes first, with the
/// investigation id breaking ties so the order is TOTAL and a page token means the
/// same thing on the call that hands it back. `Err(attribute)` means the request
/// named something that is not an `InvestigationSortField` member.
pub fn sort_investigations(
    investigations: &mut [&InvestigationRecord],
    sort: Option<&crate::ops::types::InvestigationSortCriteria>,
) -> Result<(), String> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some(ORDER_BY_ASCENDING) => false,
        Some(ORDER_BY_DESCENDING) => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named — the rule `sort_malware_scans` follows.
        _ => attribute.is_none() && DESCENDING_BY_DEFAULT,
    };
    let attribute = attribute.unwrap_or(INVESTIGATION_DEFAULT_SORT_FIELD);
    if !investigation_sort_fields().contains(&attribute) {
        return Err(attribute.to_string());
    }

    investigations.sort_by(|a, b| {
        compare_by_attribute(
            *a,
            *b,
            descending,
            |investigation| investigation_attribute(investigation, attribute),
            |investigation| investigation.id.clone(),
        )
    });
    Ok(())
}
