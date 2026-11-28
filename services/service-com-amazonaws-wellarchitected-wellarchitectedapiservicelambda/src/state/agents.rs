// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `ProfileType$STANDARD`: "a standard profile that uses an execution role and
/// aggregation configuration to analyze resources across one or more Amazon Web Services
/// accounts" (`smithy_query(query="shape", shape="ProfileType")`) — the kind
/// `CreateAgentProfile` mints.
pub const PROFILE_TYPE_STANDARD: &str = "STANDARD";

/// `ProfileType$BASIC`: "a zero-configuration profile that generates recommendations
/// without requiring an execution role or aggregation setup" — the kind
/// `CreateAgentBasicProfile` mints.
pub const PROFILE_TYPE_BASIC: &str = "BASIC";

/// `ProfileType$ORGANIZATIONAL`: "a profile that uses Amazon Web Services Organizations
/// to define its monitoring boundary" — the kind `CreateAgentOrganizationalProfile`
/// mints.
pub const PROFILE_TYPE_ORGANIZATIONAL: &str = "ORGANIZATIONAL";

/// One stored agent profile: the resource the agent-profile creates mint,
/// `GetAgentProfile` reports and the goals below belong to.
///
/// The record holds what the request carried plus what the SERVICE owns — the ARN it
/// minted, the identity it attributed the create to and when it happened — and the
/// reporting members (`eligibleFor…`, `fieldErrors`) are DERIVED from it rather than
/// stored, so they cannot drift from the profile they describe.
///
/// One record covers every `ProfileType`, because every profile read projects one shape
/// whatever kind it reports: the members a kind does not have are the ones the model
/// leaves optional and documents as "not present for basic profiles"
/// (`smithy_query(query="shape", shape="AgentProfileSummary")`), so they are held
/// optional here and reported absent for the kind that has none.
#[derive(Debug, Clone)]
pub struct AgentProfileRecord {
    pub arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub business_overview: Option<String>,
    /// The pillars the profile analyses, absent for a profile kind that names none.
    pub pillars: Option<Vec<String>>,
    pub deletion_protection: bool,
    /// Which `ProfileType` this profile is — one of [`PROFILE_TYPE_STANDARD`],
    /// [`PROFILE_TYPE_BASIC`] and [`PROFILE_TYPE_ORGANIZATIONAL`]. It is the create that
    /// decided the kind, so it is stored rather than guessed from the other members.
    pub profile_type: String,
    /// The execution role recommendation actions assume, absent for a kind that needs
    /// none.
    pub execution_role_arn: Option<String>,
    /// The accounts and Regions a STANDARD profile analyses, absent for a kind that
    /// scopes itself another way.
    pub aggregation_configuration: Option<Vec<AggregationConfiguration>>,
    /// The organizational monitoring boundary, "present only for organizational
    /// profiles".
    pub organizational_aggregation_configuration: Option<OrganizationalAggregationConfiguration>,
    /// The resource-based policy `PutAgentProfilePolicy` has attached — "the IAM
    /// resource-based policy document in JSON format" — or nothing while none is attached.
    ///
    /// A SUB-RESOURCE of the profile it governs: it is addressed as
    /// `…/{profileArn}/policy`, so it can neither be reached without the profile nor
    /// outlive it. The document is held as the caller sent it: the model constrains it as a
    /// `ResourcePolicy` string, and nothing in this service reads inside it.
    pub policy: Option<String>,
    /// The profile's tags, by key — a `BTreeMap` so they are reported in one stable
    /// order, as a workload's and a review template's tags are held.
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentProfileRecord {
    /// Whether this profile is "valid for scheduled recommendation generation" /
    /// "valid for manual architecture generation"
    /// (`CreateAgentProfileResponse$eligibleForScheduledGeneration` /
    /// `$eligibleForArchitectureGeneration`).
    ///
    /// Generation analyses the Amazon Web Services resources the profile's monitoring
    /// boundary scopes it to, so a profile whose kind takes a boundary and names nothing
    /// in it has nothing to analyse and is eligible for neither: a STANDARD profile is
    /// scoped by its `aggregationConfiguration` ("configuration settings that define the
    /// scope of Amazon Web Services resources to analyze for optimization
    /// recommendations" — `smithy_query(query="shape",
    /// shape="AggregationConfiguration")`), an ORGANIZATIONAL profile by the
    /// organizational units and accounts of its `organizationalAggregationConfiguration`,
    /// and both lists are modelled `@length(min: 0)`. A BASIC profile is
    /// "zero-configuration" (`smithy_query(query="shape", shape="ProfileType")`), so it
    /// takes no boundary and is always eligible. Both eligibility members answer the same
    /// question of the same stored scope, so they are derived from it together.
    pub fn eligible_for_generation(&self) -> bool {
        match self.profile_type.as_str() {
            PROFILE_TYPE_BASIC => true,
            PROFILE_TYPE_ORGANIZATIONAL => self
                .organizational_aggregation_configuration
                .as_ref()
                .is_some_and(|boundary| {
                    boundary
                        .organizational_units
                        .as_ref()
                        .is_some_and(|units| !units.is_empty())
                        || boundary
                            .accounts
                            .as_ref()
                            .is_some_and(|accounts| !accounts.is_empty())
                }),
            _ => self
                .aggregation_configuration
                .as_ref()
                .is_some_and(|configurations| !configurations.is_empty()),
        }
    }

    /// `fieldErrors`: one entry per input member that leaves this profile unable to
    /// generate recommendations, or `None` when none does. Derived from the stored
    /// profile, so it reports what is actually wrong with it rather than what one
    /// request looked like — and keyed by the member of the create that carries the
    /// boundary the profile's own kind is scoped by.
    pub fn field_errors(&self) -> Option<HashMap<String, String>> {
        (!self.eligible_for_generation()).then(|| {
            let member = if self.profile_type == PROFILE_TYPE_ORGANIZATIONAL {
                ORGANIZATIONAL_AGGREGATION_CONFIGURATION_MEMBER
            } else {
                AGGREGATION_CONFIGURATION_MEMBER
            };
            HashMap::from([(
                member.to_string(),
                NO_AGGREGATION_CONFIGURATION_MESSAGE.to_string(),
            )])
        })
    }

    /// The `AgentProfileSummary` projection the profile LIST reports
    /// (`ListAgentProfilesResponse$items`), so one place decides how a stored profile is
    /// summarised.
    ///
    /// The same members `CreateAgentProfile` and `GetAgentProfile` answer from the same
    /// record — the model declares `AgentProfileSummary` as its own structure
    /// (`smithy_query(query="shape", shape="AgentProfileSummary")`), so this is a third
    /// Rust type over the one stored record — with the reporting members
    /// (`eligibleFor…`, `fieldErrors`, `tags`) DERIVED from it rather than held twice, so
    /// a listed profile cannot disagree with the one a read of it reports.
    pub fn to_summary(&self) -> AgentProfileSummary {
        AgentProfileSummary {
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            business_overview: self.business_overview.clone(),
            pillars: self.pillars.clone(),
            deletion_protection: self.deletion_protection,
            profile_type: Some(self.profile_type.clone()),
            execution_role_arn: self.execution_role_arn.clone(),
            aggregation_configuration: self.aggregation_configuration.clone(),
            organizational_aggregation_configuration: self
                .organizational_aggregation_configuration
                .clone(),
            arn: self.arn.clone(),
            eligible_for_scheduled_generation: Some(self.eligible_for_generation()),
            eligible_for_architecture_generation: Some(self.eligible_for_generation()),
            field_errors: self.field_errors(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }

    /// The profile's tags as the wire carries them, or `None` when it has none: the
    /// modelled `Tags` member is a list of `Tag` key/value pairs
    /// (`smithy_query(query="shape", shape="Tag")`).
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect()
        })
    }
}

/// One stored optimization goal of an agent profile: "specific targets and objectives
/// for the optimization process" (`CreateAgentGoal` doc).
///
/// A goal belongs to the profile its `profileArn` names — it is reached only through
/// that profile's `/goals` routes — and carries, beside what the request sent, the id
/// the service minted for it and the identity and instant the create is attributed to.
#[derive(Debug, Clone)]
pub struct AgentGoalRecord {
    pub id: String,
    pub profile_arn: String,
    pub pillars: Vec<String>,
    pub title: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentGoalRecord {
    /// The `GoalSummary` projection every goal operation answers with
    /// (`CreateAgentGoal`, `GetAgentGoal`, `ListAgentGoals` and `UpdateAgentGoal`), so
    /// one place decides how a stored goal is reported.
    pub fn to_summary(&self) -> GoalSummary {
        GoalSummary {
            id: self.id.clone(),
            profile_arn: self.profile_arn.clone(),
            pillars: self.pillars.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// One stored context of an agent profile: "application and environment information used
/// during recommendation generation" (`CreateAgentContext` doc).
///
/// A context belongs to the profile its `profileArn` names — it is reached only through
/// that profile's `/contexts` routes — and carries, beside what the request sent, the id
/// the service minted for it and the identity and instant the create is attributed to.
/// Held exactly as a goal is, for the same reason: several operations reach it.
#[derive(Debug, Clone)]
pub struct AgentContextRecord {
    pub id: String,
    pub profile_arn: String,
    pub title: String,
    pub context_type: String,
    pub content: ContextContent,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentContextRecord {
    /// The `ContextSummary` projection every context operation answers with
    /// (`CreateAgentContext`, `ListAgentContexts`, `GetAgentContext` and
    /// `UpdateAgentContext`), so one place decides how a stored
    /// context is reported.
    ///
    /// `applicationType` and `criticality` are DERIVED from the stored content rather
    /// than held twice: the summary's members describe "the application described by
    /// this context" and `ContextContent` is where that application's type and
    /// criticality are recorded (`smithy_query(query="shape",
    /// shape="ContextSummary")` / `shape="ContextContent"`), so they cannot drift from
    /// the content they summarise.
    pub fn to_summary(&self) -> ContextSummary {
        ContextSummary {
            id: self.id.clone(),
            profile_arn: self.profile_arn.clone(),
            title: self.title.clone(),
            context_type: self.context_type.clone(),
            application_type: self.content.application_type.clone(),
            criticality: self.content.criticality.clone(),
            content: self.content.clone(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// `ResourceNotFoundException$ResourceType` for a recommendation generation of an agent
/// profile — the job `StartAgentRecommendationGeneration` accepts and
/// `GetAgentRecommendationGeneration` reports. `ExceptionResourceType` is a free-form
/// string, so it names the kind of resource that was missing: a generation.
pub const AGENT_GENERATION_RESOURCE_TYPE: &str = "GENERATION";

/// The most recommendation generations one agent profile holds here.
/// `StartAgentRecommendationGeneration` models `ServiceQuotaExceededException` (HTTP 402)
/// for the case; the ceiling is this emulator's own, for the reasons
/// [`MAX_AGENT_PROFILES`] gives, and it is what bounds
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::agent_generations`]
/// — nothing deletes a generation, so the map needs a bound of its own.
pub const MAX_GENERATIONS_PER_AGENT_PROFILE: usize = 100;

/// `GenerationStatus$QUEUED`: a generation this service has accepted and not started
/// working on yet (`smithy_query(query="shape", shape="GenerationStatus")`).
pub const GENERATION_QUEUED: &str = "QUEUED";

/// `GenerationStatus$IN_PROGRESS`: a generation whose analysis has started and has not
/// finished.
pub const GENERATION_IN_PROGRESS: &str = "IN_PROGRESS";

/// `GenerationStatus$COMPLETED`: the terminal status of a generation that finished —
/// "poll this operation until status reaches a terminal value of COMPLETED (results are
/// ready) or ERROR" (`GetAgentRecommendationGeneration` doc).
///
/// The enum's fourth member, `ERROR`, is the terminal status of a generation that FAILED,
/// whose `errorDetails` says why. No generation this emulator accepts reaches it — a
/// start it cannot serve is refused by `StartAgentRecommendationGeneration` instead — so
/// it is not named as a constant here; `GENERATION_STATUS_VALUES` carries the whole
/// published catalog.
pub const GENERATION_COMPLETED: &str = "COMPLETED";

/// How long after `StartAgentRecommendationGeneration` accepts a generation its analysis
/// STARTS — the span the generation spends `QUEUED`.
///
/// AWS publishes no duration for either span, so both are this emulator's own: non-zero
/// on purpose, so a caller polling straight after the start call still observes `QUEUED`,
/// and short enough not to hold a workflow up. Nothing sleeps for it —
/// [`AgentGenerationRecord::status`] compares the stored instants with the clock on every
/// read.
pub const GENERATION_QUEUE_DURATION: Duration = Duration::from_millis(250);

/// How long a generation's analysis takes once it has started — the span the generation
/// spends `IN_PROGRESS` before it reaches `COMPLETED`. This emulator's own span, for the
/// reasons [`GENERATION_QUEUE_DURATION`] gives.
pub const GENERATION_RUN_DURATION: Duration = Duration::from_millis(500);

/// `GenerationType$ONDEMAND`: the generation was started by a caller asking for one
/// (`smithy_query(query="shape", shape="GenerationType")`).
///
/// Every generation this emulator holds was started by a
/// `StartAgentRecommendationGeneration` call, so this is the type each of them reports.
/// The enum's other two members describe a generation the SERVICE starts without such a
/// call — `SCHEDULED` on a schedule, `BASIC` for a profile that generates without being
/// configured — and nothing here starts one, so neither is reported.
pub const GENERATION_TYPE_ONDEMAND: &str = "ONDEMAND";

/// One accepted recommendation generation: the asynchronous job
/// `StartAgentRecommendationGeneration` records and `GetAgentRecommendationGeneration`
/// reports the status and progress of.
///
/// The record holds what the request carried plus what the SERVICE owns — the id it
/// minted, the identity and instant it attributed the start to, and the two instants the
/// clock owes the job's transitions at. Status, progress and the transition timestamps
/// are DERIVED from those instants rather than stored, so they cannot drift from the
/// clock and no read has to write.
#[derive(Debug, Clone)]
pub struct AgentGenerationRecord {
    pub id: String,
    pub profile_arn: String,
    pub name: Option<String>,
    /// `StartAgentRecommendationGenerationRequest$types` — the kinds of recommendation
    /// this generation was started for. Stored because it is what a listing's
    /// `recommendationType` filter selects on ([`Self::covers_type`], which
    /// `ListAgentRecommendationGenerations` pages with) and what the recommendations a
    /// generation produces are of; no response shape projects it
    /// (`smithy_query(query="operation", operation="StartAgentRecommendationGeneration")`).
    pub types: Vec<String>,
    /// The scope the start request named, absent when it named none: `Scope` is modelled
    /// optional on the request, and the answer reports the scope the caller asked for.
    pub scope: Option<Scope>,
    /// The pillars this analysis covers, fixed when it was started: the scope's pillars
    /// when the request named a scope, otherwise the ones the profile itself names. A
    /// generation analyses a fixed set of pillars, so the set is recorded rather than
    /// re-derived from a profile a later update could change under it.
    pub pillars_in_scope: Vec<String>,
    pub additional_context: Option<::aws_smithy_types::Document>,
    pub created_by: String,
    pub created_at: SystemTime,
    /// When the analysis starts: `created_at` plus [`GENERATION_QUEUE_DURATION`].
    pub starts_at: SystemTime,
    /// When the analysis finishes: `starts_at` plus [`GENERATION_RUN_DURATION`].
    pub ends_at: SystemTime,
    /// The fingerprint of the request that started it, so a repeat of that request —
    /// `StartAgentRecommendationGeneration` carries `@idempotent` — resolves to this
    /// generation instead of starting a second one.
    pub fingerprint: String,
}

impl AgentGenerationRecord {
    /// The `GenerationStatus` of this generation as of `now`: `QUEUED` until its analysis
    /// starts, `IN_PROGRESS` until it finishes, then the terminal `COMPLETED`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.starts_at {
            GENERATION_QUEUED
        } else if now < self.ends_at {
            GENERATION_IN_PROGRESS
        } else {
            GENERATION_COMPLETED
        }
    }

    /// Whether this generation has reached a terminal status as of `now` — what decides
    /// whether a repeat of the request that started it resolves to it.
    pub fn is_terminal(&self, now: SystemTime) -> bool {
        self.status(now) == GENERATION_COMPLETED
    }

    /// `startedAt` — "the timestamp when the recommendation generation process started",
    /// absent while the generation is still `QUEUED`: nothing has started yet.
    pub fn started_at(&self, now: SystemTime) -> Option<SystemTime> {
        (self.starts_at <= now).then_some(self.starts_at)
    }

    /// `endedAt` — "the timestamp when the recommendation generation process completed",
    /// absent until it has.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        (self.ends_at <= now).then_some(self.ends_at)
    }

    /// `lastModifiedAt` / `lastModifiedBy`: the instant of the most recent transition the
    /// clock has made due, and the identity that made it.
    ///
    /// A generation's stored state is changed by the SERVICE moving its status, so the
    /// most recent change is the latest transition that has happened; while the generation
    /// is still `QUEUED` nothing has changed since the start call, so both members are
    /// absent rather than echoing it — the convention every create in this service
    /// follows.
    pub fn last_modified(&self, now: SystemTime) -> Option<(String, SystemTime)> {
        self.ended_at(now)
            .or_else(|| self.started_at(now))
            .map(|at| (::engine::emulator_account_id().to_string(), at))
    }

    /// `progress` — "current progress information including steps completed and completion
    /// percentage", as of `now`.
    ///
    /// A generation analyses the pillars its `scope` names ("the Well-Architected Tool
    /// Framework pillars to include in the generation scope" —
    /// `smithy_query(query="shape", shape="Scope")`), so one scoped pillar is one step of
    /// the process and `totalSteps` is how many the scope names. `stepsCompleted` is how
    /// many of them the elapsed share of [`GENERATION_RUN_DURATION`] has got through, and
    /// `completionPercentage` is that same share of the steps as a percentage, so the two
    /// members always agree.
    pub fn progress(&self, now: SystemTime) -> Progress {
        let total = i32::try_from(self.pillars_in_scope.len()).unwrap_or(i32::MAX);
        let completed = if now >= self.ends_at {
            total
        } else if now <= self.starts_at || total <= 0 {
            0
        } else {
            let elapsed = now
                .duration_since(self.starts_at)
                .unwrap_or_default()
                .as_secs_f64();
            let share = (elapsed / GENERATION_RUN_DURATION.as_secs_f64()).clamp(0.0, 1.0);
            // `share < 1.0` here, so the floor is at most `total - 1`: a generation that
            // has not ended never reports every step done.
            (share * f64::from(total)) as i32
        };
        Progress {
            steps_completed: completed,
            total_steps: total,
            completion_percentage: if total <= 0 {
                0.0
            } else {
                f64::from(completed) * 100.0 / f64::from(total)
            },
        }
    }

    /// Whether this generation was started for recommendations of `recommendation_type` —
    /// what `ListAgentRecommendationGenerations`'s optional `recommendationType` filter
    /// selects on.
    ///
    /// Asked of the stored list rather than of one value, so the filter keeps working
    /// whatever the modelled `RecommendationTypes` `@length(min: 1, max: 1)` grows to.
    pub fn covers_type(&self, recommendation_type: &str) -> bool {
        self.types.iter().any(|kind| kind == recommendation_type)
    }

    /// `generationRecommendationTypes` — "the recommendation types the generation covers"
    /// — which is the set the start request named, held on the record.
    pub fn generation_recommendation_types(&self) -> Option<Vec<String>> {
        (!self.types.is_empty()).then(|| self.types.clone())
    }

    /// The `AgentRecommendationGenerationSummary` projection a listing reports this
    /// generation as, so `ListAgentRecommendationGenerations` and
    /// `GetAgentRecommendationGeneration` cannot disagree about a generation: every
    /// member comes from the same record and the same clock-derived transitions
    /// ([`Self::status`], [`Self::last_modified`]).
    ///
    /// `estimatedCompletionTime` — "the estimated time for the generation to complete" —
    /// is the instant the clock owes the analysis its end, which is also the `endedAt`
    /// the generation reports once that instant has passed, so the estimate never
    /// contradicts the outcome.
    pub fn to_summary(&self, now: SystemTime) -> AgentRecommendationGenerationSummary {
        let last_modified = self.last_modified(now);
        AgentRecommendationGenerationSummary {
            id: self.id.clone(),
            profile_arn: self.profile_arn.clone(),
            name: self.name.clone(),
            status: self.status(now).to_string(),
            estimated_completion_time: Some(::aws_smithy_types::DateTime::from(self.ends_at)),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: last_modified
                .as_ref()
                .map(|(identity, _)| identity.clone()),
            last_modified_at: last_modified
                .map(|(_, at)| ::aws_smithy_types::DateTime::from(at)),
            generation_type: Some(GENERATION_TYPE_ONDEMAND.to_string()),
            generation_recommendation_types: self.generation_recommendation_types(),
            started_at: self
                .started_at(now)
                .map(::aws_smithy_types::DateTime::from),
            ended_at: self.ended_at(now).map(::aws_smithy_types::DateTime::from),
            // "Details about the error if the generation status is ERROR": no generation
            // this emulator records reaches `ERROR` — a start it cannot serve is refused
            // at `StartAgentRecommendationGeneration` instead — so there are no details
            // to report, and the member is absent rather than carrying an invented
            // failure.
            error_details: None,
        }
    }
}
