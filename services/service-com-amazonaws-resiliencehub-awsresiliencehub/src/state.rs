// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resiliencehub-awsresiliencehub service.

pub mod mapped_resources;

use crate::ops::types::{
    App, AppAssessment, AppComponent, FailurePolicy, LogicalResourceId, PhysicalResource,
    ResiliencyPolicy, DISRUPTION_TYPE_VALUES, ESTIMATED_COST_TIER_VALUES,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// The region every minted ARN falls back to when the request carries no usable
/// SigV4 credential scope (an unsigned, in-process call, say).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `appVersion` of an application's DRAFT version — the one version every
/// application has, and the only one the `CreateAppVersion*` / `UpdateAppVersion*`
/// operations write to (`PublishAppVersion` is what freezes a snapshot of it as
/// the current release).
pub const DRAFT_APP_VERSION: &str = "draft";

/// The `AppVersionSummary$identifier` of an application's DRAFT version: 0.
///
/// Every listed application version answers `identifier` — it is the only ordering
/// key a client has over versions, the `appVersion` label being an opaque string —
/// and the draft is the version every application starts from, so it sits below
/// every publish sequence number.
pub const DRAFT_APP_VERSION_IDENTIFIER: i64 = 0;

/// The `appVersion` that addresses an application's CURRENT RELEASE — the newest
/// snapshot `PublishAppVersion` froze from the draft.
///
/// A FIXED alias, not a number: the version LABEL and the publish sequence number
/// are distinct values in the model (`PublishAppVersion` answers both `appVersion`,
/// an `EntityVersion`, and `identifier`, a `LongOptional`), and this alias is the
/// `appVersion` every reader of a published version — `DescribeAppVersionTemplate`,
/// `ListAppVersionResources`, `DescribeAppVersionResource`,
/// `ResolveAppVersionResources` — addresses the current release by. A re-publish
/// replaces what it resolves to; see
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::publish_app_version`].
pub const RELEASE_APP_VERSION: &str = "release";

/// The most applications this emulator stores.
///
/// A MEMORY BOUND, not an AWS quota: `CreateApp` is reachable by any caller and
/// stores a caller-sized record (description, tags, event subscriptions) per call,
/// so without a ceiling a request loop grows the process until the host is out of
/// memory. Past it `CreateApp` answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when an
/// application cannot be created because a limit is reached.
pub const MAX_APPS: usize = 200;

/// The most resiliency policies this emulator stores, for the same reason as
/// [`MAX_APPS`]. `CreateResiliencyPolicy` answers `ServiceQuotaExceededException`
/// past it.
pub const MAX_RESILIENCY_POLICIES: usize = 200;

/// The most PUBLISHED app versions this emulator stores for ONE application.
///
/// A MEMORY BOUND, not an AWS quota: `PublishAppVersion` is reachable by any
/// caller and freezes a COPY of the draft's contents per call, so without a
/// ceiling a publish loop grows the process until the host is out of memory.
/// `PublishAppVersion` models no `ServiceQuotaExceededException`, so past the
/// ceiling it answers its own modelled `ValidationException` (HTTP 400) — the
/// channel its model offers for a request this service will not carry out —
/// naming the ceiling.
pub const MAX_PUBLISHED_APP_VERSIONS_PER_APP: usize = 200;

/// The most Application Components this emulator stores in ONE app version.
///
/// A MEMORY BOUND, not an AWS quota, for the same reason as [`MAX_APPS`]:
/// `CreateAppVersionAppComponent` is reachable by any caller and stores a
/// caller-sized record per call. Past it the operation answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when a
/// resource cannot be created because a limit is reached.
pub const MAX_APP_COMPONENTS_PER_APP_VERSION: usize = 200;

/// The `id` and the `name` of the AppCommon Application Component.
///
/// The one Application Component the SERVICE maintains in every app version,
/// rather than one a caller created in it: it is addressed by the same token as
/// its id and its name.
pub const APP_COMMON_COMPONENT_ID: &str = "appcommon";

/// The `type` of that component — the `AppComponent$type` value the Resilience Hub
/// grouping documentation gives the application-wide component, as against the
/// per-tier types (`AWS::ResilienceHub::StorageAppComponent` and the rest) a
/// caller picks from.
pub const APP_COMMON_COMPONENT_TYPE: &str = "AWS::ResilienceHub::AppCommonAppComponent";

/// The AppCommon Application Component of an app version.
///
/// DERIVED, never stored: this component belongs to an app version by virtue of
/// the application existing, so nothing creates it, nothing deletes it, and it
/// counts against no per-version ceiling — while every reader of a version's
/// components reports it alongside the components the version holds.
pub fn app_common_component() -> AppComponent {
    AppComponent {
        name: APP_COMMON_COMPONENT_ID.to_string(),
        r#type: APP_COMMON_COMPONENT_TYPE.to_string(),
        id: Some(APP_COMMON_COMPONENT_ID.to_string()),
        additional_info: None,
    }
}

/// The most physical resources this emulator stores in ONE app version.
///
/// A MEMORY BOUND, not an AWS quota, for the same reason as [`MAX_APPS`]:
/// `CreateAppVersionResource` is reachable by any caller and stores a
/// caller-sized record per call. Past it the operation answers its own modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the channel AWS uses when a
/// resource cannot be created because a limit is reached.
pub const MAX_RESOURCES_PER_APP_VERSION: usize = 200;

/// The most resource resolutions this emulator remembers for ONE app version,
/// oldest evicted first.
///
/// A MEMORY BOUND, not an AWS quota: `ResolveAppVersionResources` is reachable by
/// any caller and records one resolution per call, so without a ceiling a resolve
/// loop grows the process until the host is out of memory. Evicting the oldest
/// rather than refusing the call is what AWS does with resolution history — the
/// CURRENT resolution of a version is always readable — and a resolution the
/// emulator has forgotten is reported as the modelled 404 by
/// `DescribeAppVersionResourcesResolutionStatus`. Enforced in
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::start_resolution`], the only
/// writer.
pub const MAX_RESOLUTIONS_PER_APP_VERSION: usize = 50;

/// How long a resource resolution started by `ResolveAppVersionResources` takes
/// to finish.
///
/// CHOSEN, not a documented AWS duration: a real resolution's duration depends on
/// how many resources it walks. It is never zero, so a caller that reads the
/// resolution straight after starting it still sees the non-terminal status.
pub const RESOLUTION_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// How long an application assessment started by `StartAppAssessment` takes to
/// finish.
///
/// CHOSEN, not a documented AWS duration: a real assessment's duration depends on
/// how much of the application it walks. It is never zero, so a caller that reads
/// the assessment straight after starting it still sees the non-terminal status.
pub const ASSESSMENT_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The `AssessmentStatus` value an assessment that has not finished yet reports —
/// the non-terminal status `StartAppAssessment` answers with (one of
/// `ASSESSMENT_STATUS_VALUES`, the enum the model gives
/// `AppAssessment$assessmentStatus`).
pub const ASSESSMENT_STATUS_PENDING: &str = "Pending";

/// The `AssessmentStatus` value a finished assessment reports — the terminal
/// status a read finds once [`ASSESSMENT_SPAN`] has elapsed.
pub const ASSESSMENT_STATUS_SUCCESS: &str = "Success";

/// The `AssessmentInvoker` value of an assessment a caller started itself, as
/// opposed to one this service's assessment schedule started.
pub const ASSESSMENT_INVOKER_USER: &str = "User";

/// The `DriftStatus` an assessment reports for a drift check that was not run:
/// this service compares no assessment against an earlier one, so no assessment
/// reports drift it did not look for.
pub const DRIFT_STATUS_NOT_CHECKED: &str = "NotChecked";

/// The most application assessments this emulator stores for ONE application.
///
/// A MEMORY BOUND, not an AWS quota: `StartAppAssessment` is reachable by any
/// caller and records one assessment per call, so without a ceiling an assessment
/// loop grows the process until the host is out of memory. Past it
/// `StartAppAssessment` answers its own modelled `ServiceQuotaExceededException`
/// (HTTP 402) — the channel AWS uses when a request cannot be carried out because
/// a limit is reached.
pub const MAX_ASSESSMENTS_PER_APP: usize = 100;

/// The `ResourceResolutionStatusType` value a resolution that has not finished
/// yet reports — the non-terminal status `ResolveAppVersionResources` answers
/// with (one of `RESOURCE_RESOLUTION_STATUS_TYPE_VALUES`, the enum the model
/// gives `ResolveAppVersionResources$status`).
pub const RESOLUTION_STATUS_PENDING: &str = "Pending";

/// The `ResourceResolutionStatusType` value a finished resolution reports — the
/// terminal status a read finds once [`RESOLUTION_SPAN`] has elapsed.
pub const RESOLUTION_STATUS_SUCCESS: &str = "Success";

/// The modelled `@length` of `AdditionalInfoValueList` — the value list of every
/// `AdditionalInfoMap` entry — transcribed from the model (`smithy_query
/// query="shape" shape="AdditionalInfoMap"`: `value` targets
/// `AdditionalInfoValueList`, `length` 1..10). Enforced by
/// [`additional_info_violation`].
const ADDITIONAL_INFO_VALUES_MIN: usize = 1;

/// The upper half of that bound; see [`ADDITIONAL_INFO_VALUES_MIN`].
const ADDITIONAL_INFO_VALUES_MAX: usize = 10;

/// Hard ceiling on remembered idempotency tokens, oldest evicted first.
///
/// A BACKSTOP. One token is recorded per successfully created resource and
/// `DeleteApp` forgets the tokens of the app it removes, so the map is already
/// bounded by [`MAX_APPS`] + [`MAX_RESILIENCY_POLICIES`] while every creating
/// handler releases what it allocated. The ceiling is here so that invariant does
/// not depend on a future handler remembering to, and is enforced in
/// [`ComAmazonawsResiliencehubAwsresiliencehubState::record_idempotency_token`],
/// the only writer.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_APPS + MAX_RESILIENCY_POLICIES;

/// What one app version of one application holds.
///
/// The modelled shapes ARE the stored records, exactly as for `App` and
/// `ResiliencyPolicy`: a Describe is a clone of the stored value, so a read can
/// never disagree with what the last write persisted.
#[derive(Debug, Default, Clone)]
pub struct AppVersionContents {
    /// The version's Application Components, keyed by their `id`. A `BTreeMap`
    /// because a reader (`ListAppVersionAppComponents`, and any paginated read
    /// built on it) needs one stable order across calls. At most
    /// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] entries.
    pub app_components: BTreeMap<String, AppComponent>,
    /// The version's physical resources, keyed by [`logical_resource_key`] of the
    /// `logicalResourceId` they were created with — the member the model makes
    /// REQUIRED on both `CreateAppVersionResource`'s input and `PhysicalResource`,
    /// and so the one that identifies a resource within one app version. A
    /// `BTreeMap` for the same reason `app_components` is one: a reader needs one
    /// stable order across calls. At most [`MAX_RESOURCES_PER_APP_VERSION`]
    /// entries.
    pub resources: BTreeMap<String, PhysicalResource>,
    /// The `versionName` the `PublishAppVersion` call that minted this version
    /// carried, when it named one — the modelled `AppVersionSummary$versionName`.
    /// Always `None` for the draft, which nobody names.
    pub version_name: Option<String>,
    /// The modelled `AppVersionSummary$identifier` of this version: the publish
    /// sequence number a published version was minted with. `None` for the draft,
    /// which is not a published version and so has no identifier.
    pub identifier: Option<i64>,
    /// When this version was published — the modelled
    /// `AppVersionSummary$creationTime`. `None` for the draft, which comes into
    /// being with the application and whose creation time is the application's own
    /// `creationTime`.
    pub creation_time: Option<::aws_smithy_types::DateTime>,
}

impl AppVersionContents {
    /// Whether this app version's template maps at least one physical resource —
    /// the precondition `PublishAppVersion` applies to a draft before freezing
    /// it.
    ///
    /// An app template describes the RESOURCES an application is made of; its
    /// Application Components only GROUP those resources, so a version holding
    /// components and no resource maps nothing to assess.
    pub fn maps_a_resource(&self) -> bool {
        !self.resources.is_empty()
    }

    /// The resource this version holds under `logical` — the exact
    /// `logicalResourceId` it was created with.
    pub fn resource_by_logical_id(&self, logical: &LogicalResourceId) -> Option<&PhysicalResource> {
        self.resources.get(&logical_resource_key(logical))
    }

    /// The resource this version holds under the name `resource_name`, in the
    /// map's stable order.
    pub fn resource_by_name(&self, resource_name: &str) -> Option<&PhysicalResource> {
        self.resources
            .values()
            .find(|resource| resource.resource_name.as_deref() == Some(resource_name))
    }

    /// The resource this version holds for the physical identifier `identifier`,
    /// narrowed to `aws_region` and `aws_account_id` when the caller named them —
    /// the three members `DescribeAppVersionResource` addresses a resource by its
    /// physical id with.
    pub fn resource_by_physical_id(
        &self,
        identifier: &str,
        aws_region: Option<&str>,
        aws_account_id: Option<&str>,
    ) -> Option<&PhysicalResource> {
        self.resources.values().find(|resource| {
            let physical = &resource.physical_resource_id;
            physical.identifier == identifier
                && aws_region.is_none_or(|region| physical.aws_region.as_deref() == Some(region))
                && aws_account_id
                    .is_none_or(|account| physical.aws_account_id.as_deref() == Some(account))
        })
    }

    /// The Application Component of this version the caller names, by the `name`
    /// the model gives `AppComponentNameList`'s members or by the `id` it is
    /// stored under.
    pub fn app_component_named(&self, name: &str) -> Option<&AppComponent> {
        self.app_components
            .values()
            .find(|component| component.name == name || component.id.as_deref() == Some(name))
    }

    /// The Application Component of this version's whole component set the caller
    /// names — one it holds ([`Self::app_component_named`]), or the AppCommon
    /// component every app version carries, which answers to its own name and id
    /// just as a created one does.
    pub fn app_component_matching(&self, name: &str) -> Option<AppComponent> {
        if let Some(held) = self.app_component_named(name) {
            return Some(held.clone());
        }
        let implicit = app_common_component();
        (implicit.name == name || implicit.id.as_deref() == Some(name)).then_some(implicit)
    }

    /// Every Application Component of this app version: the ones created in it, in
    /// the stable order they are stored in, followed by the AppCommon component
    /// ([`app_common_component`]) every app version carries.
    pub fn app_component_set(&self) -> Vec<AppComponent> {
        let created = self.app_components.values().cloned();
        if self.app_components.contains_key(APP_COMMON_COMPONENT_ID) {
            return created.collect();
        }
        created.chain(::std::iter::once(app_common_component())).collect()
    }

    /// The Application Component of this app version addressed by `id`: one the
    /// version holds, or the AppCommon component every version carries.
    pub fn app_component_by_id(&self, id: &str) -> Option<AppComponent> {
        self.app_components
            .get(id)
            .cloned()
            .or_else(|| (id == APP_COMMON_COMPONENT_ID).then(app_common_component))
    }
    /// The resources this version holds that are IN the application — what its
    /// template's `resources` reports and what an assessment walks.
    pub fn included_resources(&self) -> Vec<&PhysicalResource> {
        self.resources
            .values()
            .filter(|resource| resource.excluded != Some(true))
            .collect()
    }

    /// The resources this version holds that are EXCLUDED from the application —
    /// what its template's `excludedResources` reports.
    pub fn excluded_resources(&self) -> Vec<&PhysicalResource> {
        self.resources
            .values()
            .filter(|resource| resource.excluded == Some(true))
            .collect()
    }
}

/// One application assessment: the record `StartAppAssessment` starts and
/// `DescribeAppAssessment` reports.
///
/// The VERDICT is computed once, from the app version and the resiliency policy as
/// they were when the assessment was started, and frozen here — a later edit to
/// either cannot change what a finished assessment reports. The STATUS is not
/// stored: it is derived from the clock in [`Self::assessment_at`], so a read never
/// has to write to move an assessment on and two readers at the same instant
/// cannot disagree about it.
#[derive(Debug, Clone)]
pub struct AppAssessmentRecord {
    /// The FINISHED assessment, verdict and all. What a reader gets before
    /// [`Self::completes_at`] is this record with everything the assessment has
    /// not finished computing withheld; see [`Self::assessment_at`].
    assessment: AppAssessment,
    /// When this assessment finishes: the instant the clock owes its move from
    /// [`ASSESSMENT_STATUS_PENDING`] to its terminal status, whether or not
    /// anybody asks.
    pub completes_at: ::std::time::SystemTime,
}

impl AppAssessmentRecord {
    /// An assessment started at `now`, finishing one [`ASSESSMENT_SPAN`] later.
    /// `assessment` is the finished record, carrying the verdict computed from
    /// what was assessed at `now`.
    pub fn started_at(assessment: AppAssessment, now: ::std::time::SystemTime) -> Self {
        Self {
            assessment,
            completes_at: now + ASSESSMENT_SPAN,
        }
    }

    /// This assessment as a reader finds it at `now`.
    ///
    /// Until the clock reaches [`Self::completes_at`] the assessment is still
    /// running, so it reports [`ASSESSMENT_STATUS_PENDING`] and NONE of the
    /// members it has not finished computing — its verdict, its score and its end
    /// time — rather than a verdict nothing has reached yet.
    pub fn assessment_at(&self, now: ::std::time::SystemTime) -> AppAssessment {
        let mut reported = self.assessment.clone();
        if now < self.completes_at {
            reported.assessment_status = ASSESSMENT_STATUS_PENDING.to_string();
            reported.end_time = None;
            reported.compliance = None;
            reported.compliance_status = None;
            reported.resiliency_score = None;
        }
        reported
    }

    /// The application this assessment is of.
    pub fn app_arn(&self) -> Option<&str> {
        self.assessment.app_arn.as_deref()
    }

    /// The tag set assigned to this assessment, when it carries one.
    pub fn tags(&self) -> Option<&HashMap<String, String>> {
        self.assessment.tags.as_ref()
    }

    /// The tag set assigned to this assessment, for writing — what the tagging
    /// operations change when the ARN they are given is an assessment's.
    pub fn tags_mut(&mut self) -> &mut Option<HashMap<String, String>> {
        &mut self.assessment.tags
    }
}

/// One resource resolution of one app version: the record
/// `ResolveAppVersionResources` starts and
/// `DescribeAppVersionResourcesResolutionStatus` reports.
///
/// The STATUS is not stored: it is derived from the clock in [`Self::status`], so
/// a read never has to write to move a resolution on and two readers at the same
/// instant cannot disagree about it.
#[derive(Debug, Clone)]
pub struct ResourceResolution {
    /// The server-generated `resolutionId` this resolution is addressed by —
    /// unique within this emulator, and never derived from the request.
    pub resolution_id: String,
    /// When this resolution finishes: the instant the clock owes its move from
    /// the non-terminal status to the terminal one, whether or not anybody asks.
    pub completes_at: ::std::time::SystemTime,
}

impl ResourceResolution {
    /// A resolution started at `now`, finishing one [`RESOLUTION_SPAN`] later.
    pub fn started_at(now: ::std::time::SystemTime) -> Self {
        Self {
            resolution_id: mint_resource_id(),
            completes_at: now + RESOLUTION_SPAN,
        }
    }

    /// This resolution's `ResourceResolutionStatusType` at `now`:
    /// [`RESOLUTION_STATUS_SUCCESS`] once the clock has reached
    /// [`Self::completes_at`], [`RESOLUTION_STATUS_PENDING`] until then.
    pub fn status(&self, now: ::std::time::SystemTime) -> &'static str {
        if now >= self.completes_at {
            RESOLUTION_STATUS_SUCCESS
        } else {
            RESOLUTION_STATUS_PENDING
        }
    }
}

/// Per-service state for com-amazonaws-resiliencehub-awsresiliencehub. A handler reaches it through [`service_state`].
///
/// The modelled `App` and `ResiliencyPolicy` shapes ARE the stored records: a
/// Describe is a clone of the stored value, so a read can never disagree with what
/// the last write persisted.
///
/// Every map here is caller-driven, so every one is bounded: `apps` by
/// [`MAX_APPS`], `app_name_index` by `apps` (one entry per stored app),
/// `policies` by [`MAX_RESILIENCY_POLICIES`], `app_versions` by `apps` (an entry
/// exists only for a stored application, and `DeleteApp` drops it) times
/// [`MAX_PUBLISHED_APP_VERSIONS_PER_APP`] plus the draft, times
/// [`MAX_APP_COMPONENTS_PER_APP_VERSION`] per version, the idempotency store
/// by [`MAX_IDEMPOTENCY_TOKENS`], `resolutions` by the same app versions
/// times [`MAX_RESOLUTIONS_PER_APP_VERSION`], and `assessments` by `apps` times
/// [`MAX_ASSESSMENTS_PER_APP`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Every stored application, keyed by its minted `appArn`. At most
    /// [`MAX_APPS`] entries.
    pub apps: HashMap<String, App>,
    /// `name` -> `appArn`. Application names are unique within an account, so this
    /// is both the name filter's index and the duplicate-name conflict detector.
    /// Exactly one entry per entry of `apps`.
    pub app_name_index: HashMap<String, String>,
    /// Every stored resiliency policy, keyed by its minted `policyArn`. At most
    /// [`MAX_RESILIENCY_POLICIES`] entries.
    pub policies: HashMap<String, ResiliencyPolicy>,
    /// What each app version of each application holds: `appArn` ->
    /// `appVersion` -> the version's contents. An entry is created only for a
    /// stored application and dropped by `DeleteApp`, so this map never outlives
    /// `apps`.
    ///
    /// The DRAFT version of a stored application exists whether or not it has an
    /// entry here — [`Self::app_version_exists`] is what answers that — so an
    /// application nobody has added anything to costs nothing.
    pub app_versions: HashMap<String, HashMap<String, AppVersionContents>>,
    /// `<operation>/<clientToken>` -> the ARN that token's FIRST call created, so a
    /// replayed `@idempotencyToken` returns the first result instead of creating a
    /// second resource. Keyed by operation as well as token because the same token
    /// may legitimately be replayed to two different operations.
    ///
    /// PRIVATE, with `record_idempotency_token` / `created_by_token` /
    /// `forget_idempotency_tokens_for` as the only access: the entry-count ceiling
    /// cannot be bypassed by a handler inserting directly.
    idempotency_tokens: HashMap<String, String>,
    /// The keys of `idempotency_tokens` in the order they were first recorded, so
    /// the ceiling evicts the oldest.
    idempotency_order: VecDeque<String>,
    /// The resource resolutions of each app version, oldest first: `appArn` ->
    /// `appVersion` -> that version's resolutions. An entry is recorded only for
    /// a stored application's existing version and dropped by
    /// [`Self::forget_app_versions`], so this map never outlives `apps`.
    ///
    /// PRIVATE, with [`Self::start_resolution`] / [`Self::resolution`] /
    /// [`Self::current_resolution`] / [`Self::resolution_mut`] as the only
    /// access: the per-version ceiling cannot be bypassed by a handler inserting
    /// directly.
    resolutions: HashMap<String, HashMap<String, VecDeque<ResourceResolution>>>,
    /// Every application assessment, keyed by its minted `assessmentArn`. An
    /// assessment is recorded only for a stored application's existing version and
    /// dropped by [`Self::forget_assessments_of`], so this map never outlives
    /// `apps`; at most [`MAX_ASSESSMENTS_PER_APP`] entries per application.
    ///
    /// PRIVATE, with [`Self::record_assessment`] / [`Self::assessment`] /
    /// [`Self::assessment_count_of`] / [`Self::forget_assessments_of`] as the only
    /// access: the per-application ceiling cannot be bypassed by a handler
    /// inserting directly.
    assessments: HashMap<String, AppAssessmentRecord>,
}

impl ComAmazonawsResiliencehubAwsresiliencehubState {
    /// Whether `app_version` of `app_arn` exists.
    ///
    /// The draft version exists for every stored application — it is created with
    /// the application and cannot be removed — while any other version exists only
    /// once something recorded it.
    pub fn app_version_exists(&self, app_arn: &str, app_version: &str) -> bool {
        if app_version == DRAFT_APP_VERSION {
            return self.apps.contains_key(app_arn);
        }
        self.app_versions
            .get(app_arn)
            .is_some_and(|versions| versions.contains_key(app_version))
    }

    /// What `app_version` of `app_arn` holds, when anything has been stored in it.
    ///
    /// `None` covers both "no such version" and "an empty draft": a reader that has
    /// to tell them apart asks [`Self::app_version_exists`] first.
    pub fn app_version(&self, app_arn: &str, app_version: &str) -> Option<&AppVersionContents> {
        self.app_versions
            .get(app_arn)
            .and_then(|versions| versions.get(app_version))
    }

    /// The draft version of `app_arn`, for writing — materialised empty on the
    /// first write to it. The caller has already established that `app_arn` names
    /// a stored application.
    pub fn draft_app_version_mut(&mut self, app_arn: &str) -> &mut AppVersionContents {
        self.app_versions
            .entry(app_arn.to_string())
            .or_default()
            .entry(DRAFT_APP_VERSION.to_string())
            .or_default()
    }

    /// The Application Component `id` addresses in `app_version` of `app_arn` —
    /// one the version holds, or the AppCommon component every app version carries
    /// ([`app_common_component`]).
    ///
    /// A version that EXISTS while holding nothing — a draft nobody has written to
    /// — still carries that component, which is why "nothing stored" is not
    /// "nothing to report" here. The caller has already established that the
    /// version exists ([`Self::app_version_exists`]); `None` means the version
    /// holds no component under that id.
    pub fn app_version_app_component(
        &self,
        app_arn: &str,
        app_version: &str,
        id: &str,
    ) -> Option<AppComponent> {
        match self.app_version(app_arn, app_version) {
            Some(contents) => contents.app_component_by_id(id),
            None => AppVersionContents::default().app_component_by_id(id),
        }
    }

    /// How many PUBLISHED versions `app_arn` has: every stored version except the
    /// draft, which is not published.
    pub fn published_app_version_count(&self, app_arn: &str) -> usize {
        self.app_versions
            .get(app_arn)
            .map(|versions| {
                versions
                    .keys()
                    .filter(|version| version.as_str() != DRAFT_APP_VERSION)
                    .count()
            })
            .unwrap_or_default()
    }

    /// The publish sequence number the next published version of `app_arn` takes:
    /// one past the highest it already has, so the first publish is 1 and the
    /// numbers never repeat for one application.
    ///
    /// Derived from the stored versions rather than from a counter, so the number
    /// a version was minted with is the number a reader finds it under.
    pub fn next_published_app_version(&self, app_arn: &str) -> i64 {
        let highest = self
            .app_versions
            .get(app_arn)
            .and_then(|versions| {
                versions
                    .values()
                    .filter_map(|contents| contents.identifier)
                    .max()
            })
            .unwrap_or(0);
        highest.saturating_add(1)
    }

    /// Store `contents` as the new CURRENT RELEASE of `app_arn`, answering the
    /// `appVersion` it is addressed by — [`RELEASE_APP_VERSION`].
    ///
    /// The snapshot the alias resolved to until now keeps its contents under its
    /// OWN publish sequence number, so a superseded release stays readable by the
    /// number it was minted with while the alias always names the newest one.
    /// `contents.identifier` is that number for the snapshot being stored.
    ///
    /// What moves with the superseded snapshot is everything recorded ABOUT that
    /// version: its contents and the resolutions recorded against it, so a
    /// resolution of the release a client resolved before re-publishing stays with
    /// the snapshot it resolved and the new release starts with none of its own.
    ///
    /// The caller has already established that `app_arn` names a stored
    /// application.
    pub fn publish_app_version(
        &mut self,
        app_arn: &str,
        contents: AppVersionContents,
    ) -> &'static str {
        let versions = self.app_versions.entry(app_arn.to_string()).or_default();

        // The key the snapshot being superseded keeps, read BEFORE it is taken out,
        // so nothing is ever removed without somewhere to put it. Only a published
        // snapshot is stored under the alias and every one of those carries the
        // sequence number it was minted with, so the key is always there.
        let superseded_key = versions
            .get(RELEASE_APP_VERSION)
            .and_then(|superseded| superseded.identifier)
            .map(|identifier| identifier.to_string());
        if let Some(key) = superseded_key {
            if let Some(superseded) = versions.remove(RELEASE_APP_VERSION) {
                versions.insert(key.clone(), superseded);
            }
            if let Some(resolutions) = self.resolutions.get_mut(app_arn) {
                if let Some(recorded) = resolutions.remove(RELEASE_APP_VERSION) {
                    resolutions.insert(key, recorded);
                }
            }
        }

        let versions = self.app_versions.entry(app_arn.to_string()).or_default();
        versions.insert(RELEASE_APP_VERSION.to_string(), contents);
        RELEASE_APP_VERSION
    }

    /// Forget every app version of `app_arn`, so deleting an application releases
    /// what its versions allocated — their contents and their resolutions alike.
    pub fn forget_app_versions(&mut self, app_arn: &str) {
        self.app_versions.remove(app_arn);
        self.resolutions.remove(app_arn);
    }

    /// `app_arn`'s application as a read finds it at `now`: the stored record,
    /// carrying the members an ASSESSMENT computes — the compliance verdict, the
    /// resiliency score and the instants they were evaluated at — from the newest
    /// assessment of this application that has finished by `now`.
    ///
    /// DERIVED on read rather than written onto the application when an assessment
    /// finishes: an assessment finishes on the clock, whether or not anybody calls,
    /// so what an application reports about its compliance cannot depend on a reader
    /// having materialised it first. An application nothing has assessed keeps the
    /// members `CreateApp` stored.
    pub fn reported_app(&self, app_arn: &str, now: ::std::time::SystemTime) -> Option<App> {
        let mut app = self.apps.get(app_arn)?.clone();
        if let Some(assessed) = self.newest_finished_assessment(app_arn, now) {
            app.compliance_status = assessed.compliance_status;
            app.last_app_compliance_evaluation_time = assessed.end_time;
            app.resiliency_score = assessed
                .resiliency_score
                .map(|score| score.score)
                .unwrap_or_default();
            app.last_resiliency_score_evaluation_time = assessed.end_time;
        }
        Some(app)
    }

    /// The assessment of `app_arn` that finished LAST of those finished by `now` —
    /// the one an application's compliance members report, because it is the most
    /// recent verdict on the application.
    fn newest_finished_assessment(
        &self,
        app_arn: &str,
        now: ::std::time::SystemTime,
    ) -> Option<AppAssessment> {
        self.assessments
            .values()
            .filter(|record| record.app_arn() == Some(app_arn) && record.completes_at <= now)
            .max_by_key(|record| record.completes_at)
            .map(|record| record.assessment_at(now))
    }

    /// Record `record` under `assessment_arn` as an assessment of one application.
    /// The caller has already established that the application is under
    /// [`MAX_ASSESSMENTS_PER_APP`].
    pub fn record_assessment(&mut self, assessment_arn: &str, record: AppAssessmentRecord) {
        self.assessments.insert(assessment_arn.to_string(), record);
    }

    /// The assessment `assessment_arn` addresses, while it is still stored.
    pub fn assessment(&self, assessment_arn: &str) -> Option<&AppAssessmentRecord> {
        self.assessments.get(assessment_arn)
    }

    /// The assessment `assessment_arn` addresses, for writing.
    pub fn assessment_mut(&mut self, assessment_arn: &str) -> Option<&mut AppAssessmentRecord> {
        self.assessments.get_mut(assessment_arn)
    }

    /// The stored assessments as a read finds them at `now` — those OF `app_arn`
    /// when one is named, every stored one otherwise, in no particular order (a
    /// reader that needs one orders them itself).
    pub fn assessments_at(
        &self,
        app_arn: Option<&str>,
        now: ::std::time::SystemTime,
    ) -> Vec<AppAssessment> {
        self.assessments
            .values()
            .filter(|record| app_arn.is_none_or(|wanted| record.app_arn() == Some(wanted)))
            .map(|record| record.assessment_at(now))
            .collect()
    }

    /// How many assessments are stored for `app_arn` — what
    /// [`MAX_ASSESSMENTS_PER_APP`] bounds.
    pub fn assessment_count_of(&self, app_arn: &str) -> usize {
        self.assessments
            .values()
            .filter(|record| record.app_arn() == Some(app_arn))
            .count()
    }

    /// Forget every assessment of `app_arn`, so deleting an application releases
    /// what assessing it allocated.
    pub fn forget_assessments_of(&mut self, app_arn: &str) {
        self.assessments
            .retain(|_, record| record.app_arn() != Some(app_arn));
    }

    /// Record a resolution of `app_version` of `app_arn`, started at `now`, as
    /// that version's CURRENT resolution, and answer it. The caller has already
    /// established that `app_arn` names a stored application and that
    /// `app_version` is one of its versions.
    ///
    /// Past [`MAX_RESOLUTIONS_PER_APP_VERSION`] the oldest resolution of that
    /// version is forgotten, so a resolve loop cannot grow this map without
    /// bound.
    pub fn start_resolution(
        &mut self,
        app_arn: &str,
        app_version: &str,
        now: ::std::time::SystemTime,
    ) -> ResourceResolution {
        let resolution = ResourceResolution::started_at(now);
        let version_resolutions = self
            .resolutions
            .entry(app_arn.to_string())
            .or_default()
            .entry(app_version.to_string())
            .or_default();
        version_resolutions.push_back(resolution.clone());
        while version_resolutions.len() > MAX_RESOLUTIONS_PER_APP_VERSION {
            version_resolutions.pop_front();
        }
        resolution
    }

    /// The resolution of `app_version` of `app_arn` that `resolution_id`
    /// addresses, while it is still remembered.
    pub fn resolution(
        &self,
        app_arn: &str,
        app_version: &str,
        resolution_id: &str,
    ) -> Option<&ResourceResolution> {
        self.version_resolutions(app_arn, app_version)?
            .iter()
            .find(|resolution| resolution.resolution_id == resolution_id)
    }

    /// The CURRENT resolution of `app_version` of `app_arn` — the one most
    /// recently started — when that version has been resolved at all.
    pub fn current_resolution(
        &self,
        app_arn: &str,
        app_version: &str,
    ) -> Option<&ResourceResolution> {
        self.version_resolutions(app_arn, app_version)?.back()
    }

    /// The resolution `resolution_id` addresses, for writing.
    pub fn resolution_mut(
        &mut self,
        app_arn: &str,
        app_version: &str,
        resolution_id: &str,
    ) -> Option<&mut ResourceResolution> {
        self.resolutions
            .get_mut(app_arn)?
            .get_mut(app_version)?
            .iter_mut()
            .find(|resolution| resolution.resolution_id == resolution_id)
    }

    /// Every remembered resolution of `app_version` of `app_arn`, oldest first.
    fn version_resolutions(
        &self,
        app_arn: &str,
        app_version: &str,
    ) -> Option<&VecDeque<ResourceResolution>> {
        self.resolutions
            .get(app_arn)
            .and_then(|versions| versions.get(app_version))
    }

    /// The ARN `operation`'s `client_token` created on its first call, if it is
    /// still remembered.
    pub fn created_by_token(&self, operation: &str, client_token: &str) -> Option<&String> {
        self.idempotency_tokens
            .get(&idempotency_key(operation, client_token))
    }

    /// Remember that `operation`'s `client_token` created `arn`, evicting the
    /// oldest remembered token once [`MAX_IDEMPOTENCY_TOKENS`] is reached. Re-recording
    /// a key already present replaces its ARN and keeps its original position, so a
    /// replay cannot be used to push other callers' tokens out.
    pub fn record_idempotency_token(&mut self, operation: &str, client_token: &str, arn: &str) {
        let key = idempotency_key(operation, client_token);
        if self
            .idempotency_tokens
            .insert(key.clone(), arn.to_string())
            .is_some()
        {
            return;
        }
        self.idempotency_order.push_back(key);
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// Forget every token that created `arn` or anything UNDER it (a sub-resource,
    /// recorded as `<arn>/<kind>/<id>`), so deleting a resource releases what
    /// creating it and its children allocated.
    pub fn forget_idempotency_tokens_for(&mut self, arn: &str) {
        let sub_resource_prefix = format!("{arn}/");
        self.idempotency_tokens
            .retain(|_, created| created != arn && !created.starts_with(&sub_resource_prefix));
        self.idempotency_order
            .retain(|key| self.idempotency_tokens.contains_key(key));
    }

    /// How many idempotency tokens are remembered. For tests and diagnostics.
    pub fn remembered_token_count(&self) -> usize {
        self.idempotency_tokens.len()
    }

    /// The tag set assigned to `resource_arn`, or `None` when this service holds
    /// no resource under that ARN.
    ///
    /// One lookup for every TAGGABLE resource this service stores — an
    /// application, a resiliency policy, an application assessment — because a
    /// tag is assigned by ARN and the operations that assign one take any of them.
    /// A resource carrying no tags answers an EMPTY map, which is what makes a
    /// read of its tags agree with the `tags` member of a read of the resource
    /// itself (see [`reported_tags`]).
    pub fn tags_of(&self, resource_arn: &str) -> Option<HashMap<String, String>> {
        let tags = if let Some(app) = self.apps.get(resource_arn) {
            app.tags.as_ref()
        } else if let Some(policy) = self.policies.get(resource_arn) {
            policy.tags.as_ref()
        } else {
            self.assessments.get(resource_arn)?.tags()
        };
        Some(tags.cloned().unwrap_or_default())
    }

    /// The tag set assigned to `resource_arn`, for writing — materialised empty
    /// when the resource carries none yet. `None` when this service holds no
    /// resource under that ARN, which is the refusal every tagging operation
    /// answers rather than tagging something that does not exist.
    pub fn tags_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        let tags = if self.apps.contains_key(resource_arn) {
            &mut self.apps.get_mut(resource_arn)?.tags
        } else if self.policies.contains_key(resource_arn) {
            &mut self.policies.get_mut(resource_arn)?.tags
        } else {
            self.assessments.get_mut(resource_arn)?.tags_mut()
        };
        Some(tags.get_or_insert_with(HashMap::new))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResiliencehubAwsresiliencehubState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResiliencehubAwsresiliencehubState>)
}

/// An Application Component as the operations whose OWN output member is the
/// component report it.
///
/// `AppComponent$additionalInfo` is the component's own attribute map, and the
/// whole component record is what such an operation answers with, so a component
/// carrying no attributes reports an EMPTY map rather than no member: a client
/// reading the component back always has a map to merge into. The embedded
/// component SUMMARIES a `PhysicalResource$appComponents` carries are the stored
/// records themselves, not this projection.
pub fn reported_app_component(component: &AppComponent) -> AppComponent {
    AppComponent {
        name: component.name.clone(),
        r#type: component.r#type.clone(),
        id: component.id.clone(),
        additional_info: Some(component.additional_info.clone().unwrap_or_default()),
    }
}

/// The `tags` a resource is persisted with: the tag map the request carried, or an
/// EMPTY map when it carried none.
///
/// `tags` is how a client reads a resource's tag state, and a resource this service
/// returns always answers it: an untagged resource carries an empty map rather than
/// dropping the member, so a client can read the map unconditionally and diff it
/// against a later read without seeing a type change. Normalising on the WRITE is
/// what makes every later read agree, because a read of an application or a
/// resiliency policy is a clone of the stored record.
pub fn reported_tags(tags: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    Some(tags.unwrap_or_default())
}

/// The most tags one resource carries: `TagMap`'s modelled `@length` maximum
/// (`smithy_query query="shape" shape="TagMap"`: length 1..50), which bounds the
/// tag set of a resource just as it bounds the map one request carries.
///
/// Also the memory bound on tagging: `TagResource` is reachable by any caller and
/// stores a caller-sized map per call, so the stored set needs a ceiling and the
/// modelled one is it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The modelled `@length` of `TagKey` — the key target of `TagMap`
/// (`smithy_query query="shape" shape="TagKey"`: length 1..128).
const TAG_KEY_MIN_LENGTH: usize = 1;

/// The upper half of that bound; see [`TAG_KEY_MIN_LENGTH`].
const TAG_KEY_MAX_LENGTH: usize = 128;

/// `TagKey`'s modelled `@pattern`, from the same shape: anything but a control
/// character or a double quote.
const TAG_KEY_PATTERN: &str = r#"^[^\x00-\x1f\x22]+$"#;

/// The refusal message for a tag key outside `TagKey`'s modelled `@length` or
/// `@pattern` — `None` when every key of `keys` is within both.
///
/// The boundary constraint validator walks a map's VALUE target but not its KEY
/// target (the same gap [`disruption_policy_violation`] covers), so a `TagMap` key
/// reaches the handler unchecked and this is where that member's constraints are
/// enforced.
/// The sentence is NOT written here: it is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, so a handler refusal and a
/// boundary refusal read identically. `TagMap` is `@sensitive`, so the violation
/// is marked sensitive and the message states the constraint without echoing the
/// key.
pub fn tag_key_violation<'a>(
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let violations: Vec<::smithy_server_runtime::Violation> = keys
        .into_iter()
        // Sorted, because a `HashMap` iterates in an arbitrary order and the same
        // request must be refused with the same sentence every time.
        .collect::<::std::collections::BTreeSet<&str>>()
        .into_iter()
        .filter_map(|key| {
            let length = key.chars().count();
            let kind = if length < TAG_KEY_MIN_LENGTH {
                ::smithy_server_runtime::ViolationKind::MinLength(TAG_KEY_MIN_LENGTH as i64)
            } else if length > TAG_KEY_MAX_LENGTH {
                ::smithy_server_runtime::ViolationKind::MaxLength(TAG_KEY_MAX_LENGTH as i64)
            } else if key.chars().any(|character| character <= '\u{1f}' || character == '"') {
                ::smithy_server_runtime::ViolationKind::Pattern(TAG_KEY_PATTERN.to_string())
            } else {
                return None;
            };
            Some(::smithy_server_runtime::Violation {
                path: member.to_string(),
                kind,
                value: None,
                sensitive: true,
            })
        })
        .collect();

    ::aws_smithy_schema_server::constraints::envelope(&violations)
}

/// The `DataLocationConstraint` a resiliency policy that names none is stored with.
///
/// The member reports the geographical constraint on where the policy's data may be
/// held, and the service resolves it on every policy rather than leaving it
/// unanswered; a caller that states no constraint has asked for none, which is what
/// this value says (one of `DATA_LOCATION_CONSTRAINT_VALUES`, the enum the model
/// gives `ResiliencyPolicy$dataLocationConstraint`).
pub const DEFAULT_DATA_LOCATION_CONSTRAINT: &str = "AnyLocation";

/// The disruption scopes a resiliency policy can state an objective for, ordered by
/// how costly an architecture recovering from that scope needs: recovering a
/// software fault needs no second copy of anything, a hardware fault a second host,
/// an Availability Zone a second zone, and a Region a second region.
///
/// Transcribed in the order the model DECLARES the values in (`smithy_query
/// query="shape" shape="DisruptionType"`: Software, Hardware, AZ, Region), which is
/// that same ascending order. The generated `DISRUPTION_TYPE_VALUES` is sorted
/// alphabetically instead, so it is not this ladder.
const DISRUPTION_SCOPE_LADDER: [&str; 4] = ["Software", "Hardware", "AZ", "Region"];

/// The `estimatedCostTier` of a resiliency policy — the member the service computes
/// itself from the policy's content, so it is answered on every policy.
///
/// What a policy costs to meet is set by the BROADEST disruption it demands recovery
/// from rather than by how tight the per-type objectives are, so the tier is the
/// cost tier sitting at the position of the policy's broadest scope in
/// [`DISRUPTION_SCOPE_LADDER`]: both ladders have four rungs, `EstimatedCostTier`'s
/// (`ESTIMATED_COST_TIER_VALUES`, L1 to L4) ascending in cost just as the scope
/// ladder ascends in the redundancy it requires.
///
/// Derived from what is stored, never remembered from a request, so a policy whose
/// disruption set changes reports the tier its current content earns.
pub fn estimated_cost_tier(policy: &HashMap<String, FailurePolicy>) -> String {
    let broadest = policy
        .keys()
        .filter_map(|disruption| {
            DISRUPTION_SCOPE_LADDER
                .iter()
                .position(|scope| scope == disruption)
        })
        .max()
        .unwrap_or(0);
    ESTIMATED_COST_TIER_VALUES[broadest.min(ESTIMATED_COST_TIER_VALUES.len() - 1)].to_string()
}

/// The key [`AppVersionContents::resources`] stores a physical resource under: its
/// whole `logicalResourceId`, which is what addresses one resource within one app
/// version.
///
/// EVERY member of the logical id takes part, each length-prefixed, so two logical
/// ids that differ only in where text falls across members (`identifier` `a/b` with
/// no stack vs `identifier` `a` in stack `b`) cannot collide on one key. A member the
/// caller left out is a distinct `-`, so an absent member and an empty one are not
/// the same key either.
pub fn logical_resource_key(logical: &LogicalResourceId) -> String {
    [
        Some(logical.identifier.as_str()),
        logical.logical_stack_name.as_deref(),
        logical.resource_group_name.as_deref(),
        logical.terraform_source_name.as_deref(),
        logical.eks_source_name.as_deref(),
    ]
    .iter()
    .map(|member| match member {
        Some(member) => format!("{}:{member}", member.len()),
        None => "-".to_string(),
    })
    .collect::<Vec<_>>()
    .join("/")
}

/// The identity `ListAppVersionResources` reports for the resource mapping of an
/// app version that no resolution of its own has been started for.
///
/// Every listing answers `resolutionId` — the model makes the member REQUIRED —
/// and an app version's mapping is resolved from the moment something is written
/// into it, whether or not a client ever asked for a resolution of that version.
/// The identity is therefore derived from what is PERSISTED: the version
/// addressed and the mapping it holds, each logical id together with the physical
/// resource and the type it maps to. Two reads of an unchanged mapping answer the
/// same identity, a mapping that changed answers a different one, two versions
/// holding the same mapping answer different ones, and no part of the value
/// depends on when it was read or on anything in the request other than the
/// version it addresses.
///
/// OPAQUE: a SHA-256 digest, so a client can read no count, instant or ordering
/// out of it, and it is well inside the modelled `@length` of `String255`.
pub fn mapping_resolution_id(
    app_arn: &str,
    app_version: &str,
    contents: Option<&AppVersionContents>,
) -> String {
    let mut digest = Sha256::new();
    absorb(&mut digest, app_arn);
    absorb(&mut digest, app_version);
    for (key, resource) in contents.into_iter().flat_map(|held| held.resources.iter()) {
        absorb(&mut digest, key);
        absorb(&mut digest, &resource.physical_resource_id.identifier);
        absorb(&mut digest, &resource.physical_resource_id.r#type);
        absorb(&mut digest, &resource.resource_type);
    }
    format!("{:x}", digest.finalize())
}

/// Feed one LENGTH-PREFIXED part into `digest`, so two mappings that differ only
/// in where text falls across parts cannot absorb the same bytes — the reason
/// [`logical_resource_key`] length-prefixes its members too.
fn absorb(digest: &mut Sha256, part: &str) {
    digest.update(format!("{}:{part}", part.len()).as_bytes());
}

/// The REGION and ACCOUNT an ARN names, each `None` when the ARN carries none
/// (a global service's ARN leaves the region empty), when `arn` is not an ARN at
/// all, or when the segment would not satisfy the model's own `AwsRegion` /
/// `CustomerId` pattern — the patterns the two members this feeds
/// (`PhysicalResourceId$awsRegion`, `$awsAccountId`) are constrained by.
pub fn arn_region_and_account(arn: &str) -> (Option<&str>, Option<&str>) {
    let mut segments = arn.split(':');
    if segments.next() != Some("arn") {
        return (None, None);
    }
    // partition, then service.
    let (_, _) = (segments.next(), segments.next());
    let region = segments
        .next()
        .filter(|region| region_matches_modeled_arn(region));
    let account = segments.next().filter(|account| {
        // `CustomerId`'s modelled pattern, `^[0-9]{12}$`.
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit())
    });
    (region, account)
}

/// The idempotency-map key for `operation`'s `client_token`.
fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// Whether `region` satisfies the REGION component of the `Arn` shape's `@pattern`,
/// transcribed from the model (`smithy_query query="shape" shape="Arn"` on
/// `smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json`, whose region
/// group is `([a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9])`): two lowercase
/// letters, a hyphen, an OPTIONAL `iso`/`iso<letter>`/`gov` infix, one or more
/// lowercase letters, a hyphen and one digit.
///
/// Every ARN this service hands back must satisfy the model's pattern, because it is
/// the same pattern the boundary validator applies when that ARN comes back in as
/// `appArn` or `policyArn` — and the region is the only part of a minted ARN the
/// caller influences.
///
/// The optional group is tried BOTH ways, exactly as the pattern's alternation does:
/// a region whose third segment merely starts with `iso` (`us-isolated-1`) matches
/// through the empty branch, so committing to the `iso` branch would reject a region
/// the pattern admits.
pub fn region_matches_modeled_arn(region: &str) -> bool {
    let Some((prefix, rest)) = region.split_once('-') else {
        return false;
    };
    if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return false;
    }

    // `((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]`: the empty branch, the `gov-`
    // branch and the `iso`/`iso<letter>-` branch.
    matches_region_tail(rest)
        || rest.strip_prefix("gov-").is_some_and(matches_region_tail)
        || iso_infix_tail(rest).is_some_and(matches_region_tail)
}

/// Whether `value` satisfies the `EntityName` shape's `@pattern`, transcribed from
/// the model (`smithy_query query="operation"
/// operation="CreateAppVersionResource"`, whose `resourceName` targets
/// `EntityName` with `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`): an alphanumeric head,
/// then 1..59 more characters out of alphanumerics, `_` and `-`.
///
/// `PhysicalResource$resourceName` targets that shape, so a name this answers
/// `false` for is one the service cannot carry on a resource.
pub fn matches_modeled_entity_name(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(head) = characters.next() else {
        return false;
    };
    if !head.is_ascii_alphanumeric() {
        return false;
    }
    let mut tail_length = 0usize;
    for character in characters {
        if !(character.is_ascii_alphanumeric() || character == '_' || character == '-') {
            return false;
        }
        tail_length += 1;
        if tail_length > 59 {
            return false;
        }
    }
    tail_length >= 1
}

/// Whether `tail` is the pattern's `[a-z]+-[0-9]` ending.
fn matches_region_tail(tail: &str) -> bool {
    let Some((letters, digit)) = tail.split_once('-') else {
        return false;
    };
    !letters.is_empty()
        && letters.bytes().all(|byte| byte.is_ascii_lowercase())
        && digit.len() == 1
        && digit.bytes().all(|byte| byte.is_ascii_digit())
}

/// What follows the pattern's `iso[a-z]{0,1}-` infix, when `rest` opens with it.
fn iso_infix_tail(rest: &str) -> Option<&str> {
    let after_iso = rest.strip_prefix("iso")?;
    if let Some(tail) = after_iso.strip_prefix('-') {
        return Some(tail);
    }
    let mut characters = after_iso.chars();
    let letter = characters.next()?;
    if !letter.is_ascii_lowercase() {
        return None;
    }
    characters.as_str().strip_prefix('-')
}

/// The region to mint this request's ARNs in: the region of the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned or scoped
/// to a region that would make the minted ARN fail the model's own `Arn` pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| region_matches_modeled_arn(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The next server-generated resource id, as the canonical 8-4-4-4-12 form AWS gives
/// a Resilience Hub application, resiliency policy or resource resolution.
///
/// OPAQUE: a fresh version-4 UUID, drawn from the operating system's random source
/// by the maintained `uuid` crate, so every nibble a client may inspect — the
/// version nibble and the variant nibble included — is the one a canonically shaped
/// identifier carries, and no part of the value encodes the minting instant, a
/// sequence number or anything else a client could read an ordering or a neighbour's
/// id out of. Two ids minted back to back are therefore unrelated, and ids of
/// different applications and policies share no derivable structure.
///
/// Unique within this emulator: 122 random bits, so the identifiers this service
/// hands out do not collide, within a run or across a restart. Server-generated,
/// never derived from the request.
fn mint_resource_id() -> String {
    ::uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The refusal message for an `additionalInfo` map whose `member` holds a value
/// list outside `AdditionalInfoValueList`'s modelled `@length` — `None` when every
/// entry is within it.
///
/// A collection's own `@length` is checked against the COLLECTION MEMBER's schema,
/// and a `map<string, list<string>>` member such as `AdditionalInfoMap` has no
/// separate schema for the inner list (the framework's own
/// `outer_element_count_bound_is_not_applied_to_a_nested_map` pins that), so the
/// boundary validator cannot reach this bound and a handler taking an
/// `AdditionalInfoMap` is where it is enforced.
///
/// The sentence is NOT written here: it is built from the framework's own
/// [`::smithy_server_runtime::Violation`] and rendered by
/// `aws_smithy_schema_server::constraints::envelope`, so a handler refusal and a
/// boundary refusal read identically — including the `<n> items` form the
/// framework displays a collection's length with.
pub fn additional_info_violation(
    member: &str,
    additional_info: &HashMap<String, Vec<String>>,
) -> Option<String> {
    // Sorted, because a `HashMap` iterates in an arbitrary order and the same
    // request must be refused with the same sentence every time.
    let mut entries: Vec<(&String, &Vec<String>)> = additional_info.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));

    let violations: Vec<::smithy_server_runtime::Violation> = entries
        .into_iter()
        .filter_map(|(key, values)| {
            let kind = if values.len() < ADDITIONAL_INFO_VALUES_MIN {
                ::smithy_server_runtime::ViolationKind::MinLength(ADDITIONAL_INFO_VALUES_MIN as i64)
            } else if values.len() > ADDITIONAL_INFO_VALUES_MAX {
                ::smithy_server_runtime::ViolationKind::MaxLength(ADDITIONAL_INFO_VALUES_MAX as i64)
            } else {
                return None;
            };
            Some(::smithy_server_runtime::Violation {
                // A map ENTRY's path embeds the caller's key, so it is elided for
                // the same reason the value is.
                path: elided(&format!(
                    "{member}.{}",
                    ::aws_smithy_schema_server::constraints::path::entry(key)
                )),
                kind,
                value: Some(format!("{} items", values.len())),
                sensitive: false,
            })
        })
        .collect();

    ::aws_smithy_schema_server::constraints::envelope(&violations)
}

/// Mint a fresh `appArn`: `arn:aws:resiliencehub:<region>:<account>:app/<id>`.
pub fn mint_app_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("app/{}", mint_resource_id()),
    )
}

/// Mint a fresh `policyArn`:
/// `arn:aws:resiliencehub:<region>:<account>:resiliency-policy/<id>`.
pub fn mint_policy_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("resiliency-policy/{}", mint_resource_id()),
    )
}

/// Mint a fresh `assessmentArn`:
/// `arn:aws:resiliencehub:<region>:<account>:app-assessment/<id>`.
pub fn mint_assessment_arn(region: &str) -> String {
    ::engine::emulator_arn(
        "resiliencehub",
        region,
        &format!("app-assessment/{}", mint_resource_id()),
    )
}

/// The namespace every Resilience Hub Application Component type is published
/// under.
///
/// `AppComponent$type` is a free-form `String255` in the model (`smithy_query
/// query="shape" shape="AppComponent"` gives it no value set), and the types the
/// service recognises are published instead: the Resilience Hub user guide's
/// "Grouping resources in an AppComponent" names one type per tier — compute,
/// database, storage, networking, message queue and the application-wide
/// [`APP_COMMON_COMPONENT_TYPE`] — and every one of them is this namespace, a
/// tier name, and [`APP_COMPONENT_TYPE_SUFFIX`]. The tier list grows as the
/// service recognises more tiers, so what is checked is the type NAMESPACE the
/// service publishes its own component types in: a type outside it — an AWS
/// resource type, or a word of the caller's own — names no Application
/// Component type at all.
const APP_COMPONENT_TYPE_NAMESPACE: &str = "AWS::ResilienceHub::";

/// The suffix every type in that namespace ends in; see
/// [`APP_COMPONENT_TYPE_NAMESPACE`].
const APP_COMPONENT_TYPE_SUFFIX: &str = "AppComponent";

/// Whether `candidate` names a Resilience Hub Application Component type — the
/// question `CreateAppVersionAppComponent` asks of the `type` it is handed, and
/// the one every writer of an `AppComponent$type` has to ask.
pub fn is_app_component_type(candidate: &str) -> bool {
    let Some(tier) = candidate.strip_prefix(APP_COMPONENT_TYPE_NAMESPACE) else {
        return false;
    };
    let Some(tier) = tier.strip_suffix(APP_COMPONENT_TYPE_SUFFIX) else {
        return false;
    };
    // A tier name, and nothing else: no empty tier, and no further namespace
    // segment hiding inside one.
    !tier.is_empty() && tier.chars().all(|character| character.is_ascii_alphanumeric())
}

/// The `ValidationException` message for a `type` that names no Application
/// Component type, naming the value the request carried.
pub fn invalid_app_component_type_message(candidate: &str) -> String {
    format!("Invalid AppComponent type: {}.", elided(candidate))
}

/// The resource types Resilience Hub supports — the catalogue a `resourceType`
/// has to name.
///
/// `CreateAppVersionResource$resourceType` is a free-form `String255` in the
/// model (`smithy_query query="operation"
/// operation="CreateAppVersionResource"`), so the set of types is published
/// rather than modelled: these are the CloudFormation type names of the
/// resources the Resilience Hub user guide's "Supported resources" lists, one
/// per supported resource of each service it assesses. A type outside the
/// catalogue names nothing this service can assess, which is why
/// `ListUnsupportedAppVersionResources` exists for the ones a resource
/// DISCOVERY finds — while a mapping a caller states explicitly is refused.
pub const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::ApiGateway::RestApi",
    "AWS::ApiGatewayV2::Api",
    "AWS::AutoScaling::AutoScalingGroup",
    "AWS::DocDB::DBCluster",
    "AWS::DocDB::DBInstance",
    "AWS::DocDB::DBSubnetGroup",
    "AWS::DynamoDB::GlobalTable",
    "AWS::DynamoDB::Table",
    "AWS::EC2::EC2Fleet",
    "AWS::EC2::Instance",
    "AWS::EC2::NatGateway",
    "AWS::EC2::Volume",
    "AWS::ECS::Cluster",
    "AWS::ECS::Service",
    "AWS::EFS::FileSystem",
    "AWS::EKS::Cluster",
    "AWS::ElastiCache::CacheCluster",
    "AWS::ElastiCache::ReplicationGroup",
    "AWS::ElasticLoadBalancing::LoadBalancer",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::FSx::FileSystem",
    "AWS::Lambda::Function",
    "AWS::RDS::DBCluster",
    "AWS::RDS::DBInstance",
    "AWS::Route53::RecordSet",
    "AWS::S3::Bucket",
    "AWS::SNS::Topic",
    "AWS::SQS::Queue",
    "AWS::StepFunctions::StateMachine",
];

/// Whether `candidate` names one of the resource types this service supports;
/// see [`SUPPORTED_RESOURCE_TYPES`].
pub fn is_supported_resource_type(candidate: &str) -> bool {
    SUPPORTED_RESOURCE_TYPES.contains(&candidate)
}

/// The `ValidationException` message for a `resourceType` outside
/// [`SUPPORTED_RESOURCE_TYPES`], naming the value the request carried.
pub fn invalid_resource_type_message(candidate: &str) -> String {
    format!("Invalid resource type: {}", elided(candidate))
}

/// The `ValidationException` message for a mapping whose physical resource the
/// service owning it says does not exist, naming the type and the identifier the
/// request carried.
///
/// A mapping is a statement that the application depends on a resource that is
/// REALLY there ([`mapped_resources`] is what asks its owner), so the refusal names
/// what could not be resolved rather than only that the request was invalid.
pub fn absent_resource_mapping_message(resource_type: &str, identifier: &str) -> String {
    format!(
        "Invalid resource mapping. The given resource does not exist. Resource Type: {}, \
         Resource Id: {}",
        elided(resource_type),
        elided(identifier)
    )
}

/// The failure types a resiliency policy has to state an objective for.
///
/// A resiliency policy is the application's recovery objective per FAILURE TYPE,
/// and the service requires one for each of the three that every application is
/// exposed to — an application (software) fault, an infrastructure (hardware)
/// fault, and the loss of an Availability Zone. `Region`, the fourth value the
/// modelled `DisruptionType` enum carries, is the multi-Region objective and is
/// stated only by a policy that has one.
///
/// Listed in the order the model declares the values in, which is also the order
/// the refusal reports a missing one in, so a policy missing several earns the same
/// refusal every time.
const REQUIRED_DISRUPTION_TYPES: [&str; 3] = ["Software", "Hardware", "AZ"];

/// The longest an RTO or an RPO objective can be: thirty days, in seconds.
///
/// The model bounds a `FailurePolicy`'s objectives only from below
/// (`smithy_query query="shape" shape="FailurePolicy"`: `rtoInSecs` and `rpoInSecs`
/// are `@range(min: 0)` integers), so the ceiling is the service's own — the
/// longest recovery objective Resilience Hub accepts for a failure type. One second
/// past it is refused.
const MAX_OBJECTIVE_SECONDS: i32 = 30 * 24 * 60 * 60;

/// The refusal message for a `DisruptionPolicy` this service will not store —
/// `None` for one it will.
///
/// The ONE place both writers of a disruption set (`CreateResiliencyPolicy` and
/// `UpdateResiliencyPolicy`) ask that question, so neither can word it
/// differently. Three rules, in the order they are judged:
///
/// * a map KEY outside the modelled `DisruptionType` names a failure type that
///   does not exist — the keys are CASE-SENSITIVE, and the boundary constraint
///   validator walks a map's VALUE target but not its KEY target, so this is
///   where that member's value set is enforced;
/// * a policy that states no objective for one of [`REQUIRED_DISRUPTION_TYPES`]
///   has no failure policy for that failure type, which is the same refusal: the
///   failure type the policy was asked for is not in it;
/// * an objective past [`MAX_OBJECTIVE_SECONDS`] is not a recovery objective this
///   service will hold, and the refusal names the failure type, which objective of
///   it, and the value the request carried.
///
/// A missing failure type and an objective's failure type are named as the
/// service's own `FailureType` vocabulary spells them — upper case — rather than as
/// the `DisruptionType` key they were sent under; a key the request carried is
/// named as it was sent.
pub fn disruption_policy_violation(policy: &HashMap<String, FailurePolicy>) -> Option<String> {
    // Sorted, so the refusal a request earns does not depend on hash order.
    let mut disruption_types: Vec<&String> = policy.keys().collect();
    disruption_types.sort();
    if let Some(unknown) = disruption_types
        .into_iter()
        .find(|disruption_type| !DISRUPTION_TYPE_VALUES.contains(&disruption_type.as_str()))
    {
        return Some(unknown_failure_type_message(unknown));
    }
    if let Some(missing) = REQUIRED_DISRUPTION_TYPES
        .iter()
        .find(|required| !policy.contains_key(**required))
    {
        return Some(unknown_failure_type_message(&missing.to_ascii_uppercase()));
    }
    // In the order the model declares the disruption types in, so a policy with
    // several objectives out of range earns the same refusal every time.
    DISRUPTION_SCOPE_LADDER.iter().find_map(|disruption_type| {
        let stated = policy.get(*disruption_type)?;
        [
            (OBJECTIVE_RTO, stated.rto_in_secs),
            (OBJECTIVE_RPO, stated.rpo_in_secs),
        ]
        .into_iter()
        .find(|(_, seconds)| *seconds > MAX_OBJECTIVE_SECONDS)
        .map(|(objective, seconds)| {
            invalid_objective_message(disruption_type, objective, seconds)
        })
    })
}

/// The two objectives a `FailurePolicy` states, as the service's own `FailureType`
/// vocabulary abbreviates them in a refusal: the recovery TIME objective and the
/// recovery POINT objective.
const OBJECTIVE_RTO: &str = "RTO";
/// See [`OBJECTIVE_RTO`].
const OBJECTIVE_RPO: &str = "RPO";

/// The `ValidationException` message for an objective outside the range one can
/// carry, naming the failure type, which of its two objectives, and the value the
/// request carried.
pub fn invalid_objective_message(disruption_type: &str, objective: &str, seconds: i32) -> String {
    format!(
        "FailureType {} {objective} {seconds} seconds is invalid",
        disruption_type.to_ascii_uppercase()
    )
}

/// The `ValidationException` message for a failure type the submitted policy has no
/// failure policy for: a key that names no failure type, or a required failure type
/// the policy does not state an objective for.
pub fn unknown_failure_type_message(disruption_type: &str) -> String {
    format!("FailureType {} does not exist", elided(disruption_type))
}

/// The longest caller-supplied text a refusal message echoes, pinned to the
/// boundary validator's own ceiling (`MAX_ECHOED_VALUE` in
/// `aws_smithy_schema_server::constraints`).
const MAX_ECHOED_VALUE: usize = 256;

/// `text` as a refusal message may carry it: itself when short, a
/// character-boundary-safe prefix with an elision when not, so a huge request
/// member cannot be amplified into a huge response.
///
/// The framework's equivalent (`constraints::echoed`) is PRIVATE and lives under
/// the agent-immutable `vendor/` tree, so it cannot be called or widened from
/// here; the ceiling is therefore pinned to the same constant above.
fn elided(text: &str) -> String {
    if text.len() <= MAX_ECHOED_VALUE {
        return text.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &text[..end], text.len())
}

/// "Now", as the wire timestamp type every creation/evaluation time uses.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

// =============================================================================
// Refusal wording
//
// The message of a refusal is part of the contract — a client branches on it —
// so each sentence this service states is written ONCE here and interpolated
// with values taken from the request and from what is stored, never from one
// recorded run. Every handler that refuses for one of these reasons calls the
// matching builder, so two operations cannot word the same refusal differently.
// =============================================================================

/// The `ResourceNotFoundException` message for an `appArn` this service holds no
/// application under — never created, or deleted since.
///
/// The ONE sentence for that refusal, whichever operation resolved the ARN: an
/// application is addressed by `appArn` by every operation that takes one
/// (`smithy_query query="digest"`), so the refusal names the member that did not
/// resolve and the value it carried.
pub fn app_not_found_message(app_arn: &str) -> String {
    format!("App not found for appArn {app_arn}")
}

/// The `ResourceNotFoundException` message for an `appVersion` the application it
/// was asked for does not have.
///
/// The APP-VERSION member of the same family as [`app_not_found_message`]: both
/// refusals reach a client as `ResourceNotFoundException`/404 on the same
/// operation, so the sentence names the entity that did not resolve — the app
/// version — and the application it was looked for in, which is what lets a
/// client probing for a version tell "no such version" from "no such
/// application". Written ONCE here because every operation that addresses an app
/// version can refuse for this reason.
pub fn app_version_not_found_message(app_arn: &str, app_version: &str) -> String {
    format!("App version {app_version} not found for appArn {app_arn}")
}

/// The `ResourceNotFoundException` message for a `resourceArn` this service holds
/// no taggable resource under — the refusal the tagging operations state, which
/// address an application, a resiliency policy or an assessment alike by the one
/// `resourceArn` member.
pub fn tagged_resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource not found for resourceArn {resource_arn}")
}

/// The `ResourceNotFoundException` message for a `policyArn` this service holds no
/// resiliency policy under.
///
/// The ONE sentence for that refusal, whichever operation resolved the ARN — the
/// entity is named as the model names its shape (`ResiliencyPolicy`).
pub fn policy_not_found_message(policy_arn: &str) -> String {
    format!("ResiliencyPolicy {policy_arn} not found")
}

/// The `ResourceNotFoundException` message for a request that cannot ATTACH the
/// resiliency policy it names to an application, because no policy is stored
/// under that ARN.
///
/// Distinct from [`policy_not_found_message`]: the request's subject is the
/// application, not the policy, so the refusal states what it could not do and
/// names BOTH the policy ARN the request carried and the application it was being
/// attached to — without which a client batching such calls cannot tell which one
/// failed from the error alone.
///
/// The sentence terminates with a period followed by a single apostrophe: the
/// service states this refusal with that trailing character, so it belongs to the
/// FIXED text of the class and not to any one request. Only the policy ARN (twice)
/// and the application's name vary, both taken from the request being refused.
pub fn policy_attachment_not_found_message(policy_arn: &str, app_name: &str) -> String {
    format!(
        "Cannot attach policy {policy_arn} to app {app_name}. \
         Policy {policy_arn} does not exist.'"
    )
}

/// The `ConflictException` message for a `CreateApp` whose `name` another
/// application of this account already answers to.
///
/// An application's name identifies it within the account, so the refusal names
/// the NAME the request asked for rather than the ARN of the application holding
/// it: the name is what the caller has to change.
pub fn duplicate_app_name_message(name: &str) -> String {
    format!("App with name {name} already exists.")
}

/// The `ConflictException` message for a resource the app template being composed
/// already holds.
///
/// The refusal names the duplicated resource by the id the template addresses it
/// by and states the remediation — editing the template — because that, and not a
/// retry, is what clears it. The id is [`elided`] first: it is a caller-supplied
/// value of up to 255 characters (`LogicalResourceId$identifier` targets
/// `String255`), so echoing it whole would amplify one request into a larger
/// refusal.
pub fn duplicate_template_resource_message(resource_id: &str) -> String {
    format!(
        "Resource with id {} exists in the current app template. \
         Please update the the app template to remove the duplicated resource.",
        elided(resource_id)
    )
}

/// The wire error code this service refuses an invalid request with — the
/// client-input error its model declares for every operation (`ValidationException`,
/// `httpError` 400).
pub const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// This service's constraint report for a request whose values do not satisfy the
/// `@pattern` of the members they were sent for: a BRACKETED LIST with one entry
/// per violated member, `None` when the refusal is about something else.
///
/// The service reports a pattern violation as the regular-expression engine states
/// it — the expression the value had to match, evaluated as an ECMA-262 one, and
/// the value that did not match it — rather than in the common AWS framework form
/// that counts the errors and names the member. Both come from this request: the
/// expression from the model member the value was sent for, the value from the
/// request itself (already elided by the framework before it reaches here).
///
/// The report itself is the shared one
/// (`service_common::validation_message::pattern_constraint_report`), which the V2
/// endpoint of this same service states too, so the two cannot drift; what stays here
/// is WHICH class this service rewords.
///
/// Only the pattern class is reworded. A violation of any OTHER modelled
/// constraint keeps the framework's own wording, which is also what this service's
/// handler-side refusals of a modelled constraint state (see
/// [`additional_info_violation`] and [`tag_key_violation`]), so a request that
/// breaks a non-pattern rule reads the same whichever layer judged it.
pub fn pattern_constraint_report(violations: &[::smithy_server_runtime::Violation]) -> Option<String> {
    ::service_common::validation_message::pattern_constraint_report(violations)
}
