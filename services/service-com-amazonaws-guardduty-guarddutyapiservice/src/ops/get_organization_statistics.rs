// SPDX-License-Identifier: CC0-1.0

//! @op GetOrganizationStatistics
//! IMPLEMENTED: reports the organization's GuardDuty coverage, counted off the
//! membership and the feature switches this service persists.
//!
//! "Retrieves how many active member accounts have each feature enabled within
//! GuardDuty" (`smithy_query(query="doc",shape="GetOrganizationStatistics")`), so every
//! number below is a COUNT over stored records, never a canned figure:
//!
//! * `totalAccountsCount` — "total number of accounts in your organization": the
//!   administrator's own account plus one per member record it holds.
//! * `memberAccountsCount` — the accounts "that are associated with GuardDuty": the
//!   member records that are still ASSOCIATED
//!   ([`crate::state::MemberRecord::is_associated`]), which is what `DisassociateMembers`
//!   ends and `CreateMembers` begins.
//! * `activeAccountsCount` — the "active accounts" among them: a member whose monitoring
//!   `StopMonitoringMembers` has stopped ("stops GuardDuty monitoring for the specified
//!   member accounts") is not active, so it is the associated members that are not
//!   [`crate::state::RELATIONSHIP_DISABLED`].
//! * `enabledAccountsCount` — "total number of accounts that have enabled GuardDuty":
//!   those monitored members, plus the administrator's own account when its detector's
//!   `Status` is [`crate::state::ENABLED`].
//! * `countByFeature` — "the coverage statistics for each feature", so one entry per
//!   `OrgFeature` member ([`crate::state::ORG_FEATURES`]), each carrying how many of the
//!   ACTIVE member accounts hold that feature's switch at
//!   [`crate::state::ENABLED`] — what `UpdateMemberDetectors` writes and
//!   `GetMemberDetectors` reports.
//! * `updatedAt` — "the timestamp at which the organization statistics was last
//!   updated": the latest instant any record these counts read last changed, so it moves
//!   when the answer can have moved and never otherwise. Absent while the endpoint holds
//!   no detector at all, because then nothing has been recorded to have an instant.
//!
//! A pure report: it takes the store's guard, counts, and releases it, so a later caller
//! sees exactly what this one saw. The operation names no detector
//! (`GET /organization/statistics`), so the organization it reports is the one the Region
//! the request was signed for serves — the same resolution `GetInvitationsCount` makes,
//! for the same reason: a GuardDuty detector is a regional resource.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so it
//! had no handler file, no place in the service's `operations(...)` allow-list and was
//! answered by the framework's unroutable-operation path — a 501 in no shape this
//! service's model declares. `build.rs` derives that allow-list from the `//! @op <Name>`
//! header of every `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back; never
//! construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`. The
//! shapes only THIS operation reaches are declared below rather than under `types/`,
//! because that module's `mod.rs` is rendered from the model and declares a submodule
//! only for an operation the generator emitted a stub for — never for a hand-authored
//! one, so a model update leaves such a submodule undeclared and unreachable.
//!
//! ---
//!
//! # Modeled contract for `GetOrganizationStatistics`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json")
//! ```
//!
//! Retrieves how many active member accounts have each feature enabled within GuardDuty.
//! Only a delegated GuardDuty administrator of an organization can run this API. When you
//! create a new organization, it might take up to 24 hours to generate the statistics for
//! the entire organization.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members. Bound to `GET /organization/statistics`.
//!
//! ## Output
//!
//! * `OrganizationDetails` (OrganizationDetails) — Information about the statistics report
//!   for your organization.
//!   * `UpdatedAt` (Timestamp) — The timestamp at which the organization statistics was
//!     last updated. This is in UTC format.
//!   * `OrganizationStatistics` (OrganizationStatistics) — Information about the GuardDuty
//!     coverage statistics for members in your organization.
//!     * `TotalAccountsCount` (Integer) — Total number of accounts in your organization.
//!     * `MemberAccountsCount` (Integer) — Total number of accounts in your organization
//!       that are associated with GuardDuty.
//!     * `ActiveAccountsCount` (Integer) — Total number of active accounts in your
//!       organization that are associated with GuardDuty.
//!     * `EnabledAccountsCount` (Integer) — Total number of accounts that have enabled
//!       GuardDuty.
//!     * `CountByFeature` (OrganizationFeatureStatisticsResults) — Retrieves the coverage
//!       statistics for each feature.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — A bad request exception object.
//! * `InternalServerErrorException` (HTTP 500) — An internal server error exception object.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetOrganizationStatisticsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetOrganizationStatisticsError` alias below.
#[smithy_macros::operation_error("com.amazonaws.guardduty#GetOrganizationStatistics")]
#[derive(Debug)]
pub enum GetOrganizationStatisticsErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerErrorException(InternalServerErrorException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_organization_statistics::GetOrganizationStatisticsError`).
pub type GetOrganizationStatisticsError =
    crate::scaffolded::Scaffolded<GetOrganizationStatisticsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetOrganizationStatisticsErrorModeled> for GetOrganizationStatisticsError {
    fn from(modeled: GetOrganizationStatisticsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `GET_ORGANIZATION_STATISTICS_INPUT_SCHEMA` static and both
/// the serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
///
/// This shape and the output tree below live in THIS file, not in a `types/` submodule,
/// because `types/mod.rs` is rendered from the model by the scaffold generator and
/// declares a submodule only for an operation it emitted a stub for. It emits none for
/// this operation (Unit input, `src/ops/SKIPPED_STUBS.md`), so a hand-written
/// `types/get_organization_statistics_types.rs` was left undeclared — and therefore
/// unreachable — by the next model update. A hand-authored operation keeps the shapes
/// only it reaches beside its handler, where no generator rewrites them.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetOrganizationStatisticsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#GetOrganizationStatisticsResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetOrganizationStatisticsOutput {
    pub organization_details: Option<OrganizationDetails>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationDetails")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationDetails {
    pub updated_at: Option<::aws_smithy_types::DateTime>,
    pub organization_statistics: Option<OrganizationStatistics>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationStatistics")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationStatistics {
    pub total_accounts_count: Option<i32>,
    pub member_accounts_count: Option<i32>,
    pub active_accounts_count: Option<i32>,
    pub enabled_accounts_count: Option<i32>,
    pub count_by_feature: Option<Vec<OrganizationFeatureStatistics>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationFeatureStatistics")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationFeatureStatistics {
    pub name: Option<String>,
    pub enabled_accounts_count: Option<i32>,
    pub additional_configuration: Option<Vec<OrganizationFeatureStatisticsAdditionalConfiguration>>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.guardduty#OrganizationFeatureStatisticsAdditionalConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct OrganizationFeatureStatisticsAdditionalConfiguration {
    pub name: Option<String>,
    pub enabled_accounts_count: Option<i32>,
}

/// The `GetOrganizationStatistics` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetOrganizationStatistics<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetOrganizationStatistics<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetOrganizationStatistics<Eng>
{
    type Input = GetOrganizationStatisticsInput;
    type Output = GetOrganizationStatisticsOutput;
    type Error = GetOrganizationStatisticsError;

    const NAME: &'static str = "GetOrganizationStatistics";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_ORGANIZATION_STATISTICS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        GET_ORGANIZATION_STATISTICS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing on the request to read
        // beyond the scope it was signed for.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetOrganizationStatisticsErrorModeled::InternalServerErrorException(
                    crate::state::state_unavailable(),
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx);

        // A pure report: nothing below writes to the store.
        let state = state.lock();
        let organization = Organization::of(&state, &region);

        Ok(GetOrganizationStatisticsOutput {
            organization_details: Some(OrganizationDetails {
                // Absent while the endpoint holds nothing, rather than stamped with the
                // instant of this call: a report never invents the instant a record
                // changed.
                updated_at: organization
                    .last_changed_millis
                    .map(::aws_smithy_types::DateTime::from_millis),
                organization_statistics: Some(OrganizationStatistics {
                    total_accounts_count: Some(count(organization.total_accounts)),
                    member_accounts_count: Some(count(organization.associated_members)),
                    active_accounts_count: Some(count(organization.active_members.len())),
                    enabled_accounts_count: Some(count(organization.enabled_accounts)),
                    // One entry per `OrgFeature` member, in the order this crate's
                    // model-derived catalog holds them, so a feature the model gains is
                    // reported without touching this file and two reads of one store
                    // never disagree about the order.
                    count_by_feature: Some(
                        crate::state::ORG_FEATURES
                            .iter()
                            .map(|name| feature_statistics(name, &organization.active_members))
                            .collect(),
                    ),
                }),
            }),
        })
    }
}

/// The organization one endpoint serves, reduced to what the statistics report.
///
/// Built in ONE walk of the store so every number answers about the same snapshot: a
/// second walk could see a membership another request had moved in between.
struct Organization<'a> {
    /// "Total number of accounts in your organization": the administrator's own account
    /// plus one per member record it holds, whatever state that record is in — a
    /// disassociated account is still an account of the organization, which is why the
    /// record outlives the association.
    total_accounts: usize,
    /// The member accounts "associated with GuardDuty".
    associated_members: usize,
    /// The ACTIVE member accounts, each as the feature switches stored for it: the
    /// accounts `countByFeature` counts.
    active_members: Vec<&'a [crate::state::FeatureRecord]>,
    /// "Total number of accounts that have enabled GuardDuty": the active member
    /// accounts, which are the ones GuardDuty is monitoring, plus the administrator's own
    /// account when its detector is enabled.
    ///
    /// A member account's own detector is not a resource this emulator holds — the stored
    /// member carries a relationship and a feature configuration, not a detector status —
    /// so whether the administrator is monitoring it is the only record there is of its
    /// GuardDuty being on, and that is what this counts.
    enabled_accounts: usize,
    /// The latest instant any record above last changed, or `None` when there is no
    /// record at all.
    last_changed_millis: Option<i64>,
}

impl<'a> Organization<'a> {
    /// Walk the detectors `region`'s endpoint serves and reduce them to the counts.
    ///
    /// The organization is resolved from the Region the request was signed for because
    /// the operation names no detector, exactly as `GetInvitationsCount` resolves the
    /// inbox it counts. The Region is the whole key: this emulator serves ONE account
    /// ([`crate::state::administrator_account_id`]), every detector in the store belongs
    /// to it, and a detector is one per account per Region — so the Region's detector is
    /// the administrator's, and the members hanging off it are its organization's.
    fn of(
        state: &'a crate::state::ComAmazonawsGuarddutyGuarddutyapiserviceState,
        region: &str,
    ) -> Self {
        let mut organization = Self {
            total_accounts: 0,
            associated_members: 0,
            active_members: Vec::new(),
            enabled_accounts: 0,
            last_changed_millis: None,
        };

        for detector in state
            .detectors
            .values()
            .filter(|detector| detector.region == region)
        {
            // The administrator's own account is one of the organization's accounts, and
            // its detector's `Status` is whether GuardDuty is enabled for it.
            organization.total_accounts += 1;
            if detector.status == crate::state::ENABLED {
                organization.enabled_accounts += 1;
            }
            organization.observe(detector.updated_at_millis);
            organization.observe_features(&detector.features);

            for member in detector.members.values() {
                organization.total_accounts += 1;
                organization.observe(member.updated_at_millis);
                organization.observe_features(&member.features);
                if !member.is_associated() {
                    continue;
                }
                organization.associated_members += 1;
                if member.relationship_status == crate::state::RELATIONSHIP_DISABLED {
                    continue;
                }
                organization.active_members.push(&member.features);
                organization.enabled_accounts += 1;
            }
        }

        organization
    }

    /// Keep `millis` when it is the latest instant seen so far.
    fn observe(&mut self, millis: i64) {
        self.last_changed_millis = Some(
            self.last_changed_millis
                .map_or(millis, |held| held.max(millis)),
        );
    }

    /// [`Self::observe`] for a feature list: a feature switch carries its own instant, so
    /// configuring one moves the statistics without moving the record that holds it.
    fn observe_features(&mut self, features: &[crate::state::FeatureRecord]) {
        for feature in features {
            self.observe(feature.updated_at_millis);
            for additional in &feature.additional_configuration {
                self.observe(additional.updated_at_millis);
            }
        }
    }
}

/// One `OrgFeature`'s entry in `countByFeature`: how many of `accounts` hold its switch
/// at [`crate::state::ENABLED`], and the same count for each additional configuration
/// stored under it.
///
/// An additional configuration takes effect only within the feature it hangs off, so an
/// account counts under one only when BOTH switches are enabled — which is what keeps
/// every nested count at or below the feature's own. A configuration no counted account
/// stores is ABSENT rather than reported as zero: nothing is invented for a switch
/// nobody has set, the rule `DescribeOrganizationConfiguration` follows for the same
/// nested list.
fn feature_statistics(
    name: &str,
    accounts: &[&[crate::state::FeatureRecord]],
) -> OrganizationFeatureStatistics {
    let mut enabled_accounts: usize = 0;
    // Ordered by configuration name, so two reads of one store report them alike.
    let mut additional: ::std::collections::BTreeMap<&str, usize> =
        ::std::collections::BTreeMap::new();

    for features in accounts {
        let Some(feature) = features.iter().find(|feature| feature.name == name) else {
            continue;
        };
        let feature_enabled = feature.status == crate::state::ENABLED;
        if feature_enabled {
            enabled_accounts += 1;
        }
        for configuration in &feature.additional_configuration {
            let counted = additional.entry(configuration.name.as_str()).or_insert(0);
            if feature_enabled && configuration.status == crate::state::ENABLED {
                *counted += 1;
            }
        }
    }

    OrganizationFeatureStatistics {
        name: Some(name.to_string()),
        enabled_accounts_count: Some(count(enabled_accounts)),
        additional_configuration: if additional.is_empty() {
            None
        } else {
            Some(
                additional
                    .into_iter()
                    .map(
                        |(name, accounts)| OrganizationFeatureStatisticsAdditionalConfiguration {
                            name: Some(name.to_string()),
                            enabled_accounts_count: Some(count(accounts)),
                        },
                    )
                    .collect(),
            )
        },
    }
}

/// One count, as the modelled `Integer` members carry it.
///
/// Every count here is bounded by the store's own member quota times the number of
/// Regions it partitions into, so the saturation below is unreachable for anything this
/// store can hold; it is there so a report can never wrap.
fn count(counted: usize) -> i32 {
    i32::try_from(counted).unwrap_or(i32::MAX)
}
