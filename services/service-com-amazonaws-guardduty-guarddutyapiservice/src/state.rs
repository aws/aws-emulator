// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-guardduty-guarddutyapiservice service.
//!
//! GuardDuty's control plane is rooted at the *detector*: one per account per
//! Region. Everything the detector owns — its findings, its filters and its threat
//! lists — hangs off the [`Detector`] record, so `DeleteDetector` cascades by
//! dropping one map entry.
//!
//! A Malware Protection plan is NOT one of those: it is created against an S3
//! bucket rather than against a detector, so [`MalwareProtectionPlan`] hangs off
//! the service's own map and outlives any detector.
//!
//! This module root holds the state struct itself plus the helpers more than one
//! concern needs (id minting, the monotonic clock, the request's Region and the
//! shared refusal wording); each resource family's records and rules live in a
//! child module beside it. Each operation builds its own modelled output in its
//! own `src/ops/<op>.rs`.

/// The Runtime Monitoring coverage of the account's compute resources, which more than
/// one operation reports on.
pub mod coverage;

/// The published custom detection rule catalogue and the associations an account
/// makes against one.
mod custom_detection_rules;
/// The detector record and the member accounts, administrator and invitations
/// around it.
mod detectors;
/// The trusted- and threat-entity lists a detector holds.
mod entity_sets;
/// Detector features and the data sources behind them.
mod features;
/// Findings, the criteria that select them and the saved finding filter.
mod findings;
/// AI-analyst investigations.
mod investigations;
/// The trusted-IP lists a detector holds.
mod ip_sets;
/// The IAM role a Malware Protection plan works under, as the account's IAM
/// records answer for it.
mod malware_protection_plan_role;
/// Malware Protection plans.
mod malware_protection_plans;
/// Malware scans and the reports over them.
mod malware_scans;
/// The delegated administrator and the organization-wide configuration.
mod organization;
/// The listing a page token names a position in, and the page cut itself.
mod pagination;
/// The export target a detector publishes its findings to.
mod publishing_destinations;
/// Resource tagging.
mod tagging;
/// The threat lists a detector holds.
mod threat_intel_sets;

// Every child module's items are part of this module's surface: `crate::state::X`
// resolves whichever child holds `X`, so an operation names the state it reads
// without knowing which file it lives in.
pub use custom_detection_rules::*;
pub use detectors::*;
pub use entity_sets::*;
pub use features::*;
pub use findings::*;
pub use investigations::*;
pub use ip_sets::*;
pub use malware_protection_plan_role::*;
pub use malware_protection_plans::*;
pub use malware_scans::*;
pub use organization::*;
pub use pagination::*;
pub use publishing_destinations::*;
pub use tagging::*;
pub use threat_intel_sets::*;

use std::collections::{BTreeMap, HashMap};

/// Per-service state for com-amazonaws-guardduty-guarddutyapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// Every persisted detector, keyed by its minted `DetectorId`.
    ///
    /// BOUNDED by [`MAX_DETECTOR_REGIONS`]: the store holds at most one detector
    /// per Region ("you can have only one detector per account per Region"), and
    /// the Region comes from the request's credential scope, so the number of
    /// PARTITIONS needs its own ceiling — see
    /// [`Self::detector_regions_exhausted`], which `CreateDetector` refuses past.
    /// Everything a detector owns is bounded in turn
    /// ([`MAX_MEMBERS_PER_DETECTOR`], [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`],
    /// [`IP_SETS_PER_DETECTOR`] plus [`DELETED_IP_SETS_RETAINED`],
    /// [`THREAT_INTEL_SETS_PER_DETECTOR`],
    /// [`THREAT_ENTITY_SETS_PER_DETECTOR`]), so the whole map is finite whatever a
    /// caller sends.
    pub detectors: HashMap<String, Detector>,
    /// `ClientToken` -> the detector a `CreateDetector` minted under it, so a
    /// replayed create returns that detector instead of refusing or minting a second
    /// one. Keyed by `(region, token)`: an idempotency token is scoped to the
    /// endpoint it was replayed against.
    ///
    /// BOUNDED by [`MAX_CLIENT_TOKENS`]. The token is REQUEST-chosen, so a caller
    /// replaying creates with fresh tokens could otherwise grow this map without
    /// limit — the same hazard [`MAX_MEMBERS_PER_DETECTOR`] guards for
    /// request-chosen account ids. Write it through
    /// [`Self::record_client_token`], which drops the oldest mappings and the ones
    /// whose detector is gone, and read it through
    /// [`Self::detector_for_client_token`]; never insert into it directly.
    pub client_tokens: HashMap<(String, String), ClientTokenRecord>,
    /// Every persisted Malware Protection plan, keyed by its minted
    /// `MalwareProtectionPlanId`.
    ///
    /// A plan is NOT owned by a detector: Malware Protection for S3 is created
    /// against an S3 bucket ("Creates a new Malware Protection plan for the
    /// protected resource", `smithy_query(query="doc",shape="CreateMalwareProtectionPlan")`),
    /// so it hangs off the service rather than off [`Detector`].
    ///
    /// BOUNDED at [`MAX_MALWARE_PROTECTION_PLANS_TOTAL`] records: the per-Region
    /// quota bounds one Region's plans and
    /// [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`] bounds how many Regions the store
    /// partitions into, both of which `CreateMalwareProtectionPlan` enforces before
    /// it writes.
    pub malware_protection_plans: HashMap<String, MalwareProtectionPlan>,
    /// `ClientToken` -> `MalwareProtectionPlanId`, the plan counterpart of
    /// [`Self::client_tokens`], keyed by `(region, token)` for the same reason.
    ///
    /// Separate from the detector index on purpose: a token resolves to a resource
    /// of the type the create that carried it made, so one token replayed against
    /// two different creates must never hand back the other one's id.
    ///
    /// The token is a pure REPLAY key: it maps to the plan it minted and nothing
    /// about the replaying request is compared against it, so a replay carrying
    /// different members answers with that plan unchanged rather than minting a
    /// second one or mutating the first. Creating is not updating —
    /// `UpdateMalwareProtectionPlan` is the operation that changes a plan — and a
    /// replay that did mutate would make a retried create a hidden update.
    ///
    /// It is BOUNDED by [`MAX_CLIENT_TOKENS`], the ceiling the detector index
    /// carries and for the same reason: the key is request-chosen, so an entry per
    /// create-then-delete cycle would otherwise accumulate for ever (the plan quota
    /// a delete frees bounds the PLANS, not the mappings that named them). Write it
    /// through [`Self::record_malware_protection_plan_client_token`], which drops
    /// the mappings whose plan is gone and then the oldest, read it through
    /// [`Self::malware_protection_plan_for_client_token`], which resolves the plan
    /// so a stale mapping yields nothing, and drop a deleted plan's mappings with
    /// [`Self::forget_malware_protection_plan_client_tokens_for`]; never insert
    /// into it directly.
    pub malware_protection_plan_client_tokens:
        HashMap<(String, String), MalwareProtectionPlanTokenRecord>,
    /// Every malware scan this service holds, keyed by its minted `ScanId`.
    ///
    /// A scan is NOT owned by a detector: `StartMalwareScan` names the resource to
    /// scan by ARN and carries no `DetectorId`
    /// (`smithy_query(operation="StartMalwareScan")`), so a scan hangs off the
    /// service and survives a `DeleteDetector` the way a
    /// [`MalwareProtectionPlan`] does. It is still REGIONAL — the Region the start
    /// call was signed for is the only endpoint that reports it, which
    /// [`Self::malware_scan_in`] enforces.
    ///
    /// BOUNDED at [`MAX_MALWARE_SCANS_PER_REGION`] scans per Region across at most
    /// [`MAX_MALWARE_SCAN_REGIONS`] Regions, both of which `StartMalwareScan`
    /// enforces before it writes ([`Self::malware_scan_count_in`],
    /// [`Self::malware_scan_regions_exhausted`]). The `ClientToken` a start carried
    /// lives ON the scan record ([`MalwareScan::client_token`]) rather than in an
    /// index of its own, so the idempotency mappings are bounded by the scan store
    /// itself and no mapping can outlive the scan it names.
    pub malware_scans: HashMap<String, MalwareScan>,
    /// GuardDuty's own catalogue of custom detection rules, materialised when this
    /// service's store is created.
    ///
    /// A rule is the SERVICE's, not an account's: the API models no operation that
    /// creates, updates or deletes one — `ListCustomDetectionRules` "Returns all
    /// available custom detection rules in GuardDuty"
    /// (`smithy_query(query="doc",shape="ListCustomDetectionRules")`), and the only
    /// writes it models against a rule are the ASSOCIATIONS an account makes
    /// (`CreateCustomDetectionRuleAssociation`). So the catalogue is seeded once,
    /// reads the same for every account and every Region, and is FINITE by
    /// construction: it is the fixed table behind
    /// [`CustomDetectionRuleCatalog::default`].
    pub custom_detection_rules: CustomDetectionRuleCatalog,
    /// Every custom detection rule ASSOCIATION this account has made, keyed by
    /// `(region, RuleId)`.
    ///
    /// An association binds a PUBLISHED rule
    /// ([`Self::custom_detection_rules`]) to this account, so the RULE identifies
    /// it: a second create for a rule the account already associated is the
    /// `ConflictException` the operation models rather than a second record,
    /// whatever execution mode either names, and
    /// `UpdateCustomDetectionRuleAssociation` is what moves the one association a
    /// rule has between modes. The `AssociationId` the model also carries is a
    /// minted handle onto that rule's association, resolved by
    /// [`Self::detection_rule_association_by_id`].
    ///
    /// BOUNDED by construction: a Region holds at most one association per
    /// published rule — a fixed catalogue — across at most
    /// [`MAX_DETECTION_RULE_REGIONS`] Regions, which the create enforces before it
    /// writes ([`Self::detection_rule_regions_exhausted`]). The `ClientToken` a
    /// create carried lives ON the record rather than in an index of its own, so
    /// the idempotency mappings are bounded by the same ceiling.
    pub custom_detection_rule_associations:
        BTreeMap<(String, String), DetectionRuleAssociationRecord>,
    /// Every custom detection rule ORGANIZATION configuration this account has
    /// created, keyed by `(region, RuleId, Mode)` — the Region for the same reason
    /// [`Self::custom_detection_rule_associations`] is keyed by it.
    ///
    /// Separate from the associations on purpose: an association enrolls THIS
    /// account in a rule, while an organization configuration says which member
    /// accounts the organization enrolls, so one `(RuleId, Mode)` pair can carry
    /// either, both or neither, and a read of one must never answer with the
    /// other.
    ///
    /// BOUNDED exactly as the association map is, by the same create-time guard.
    pub custom_detection_rule_org_configurations:
        BTreeMap<(String, String, String), DetectionRuleOrgConfigurationRecord>,
    /// The GuardDuty delegated administrator the organization has, keyed by the
    /// Region the designating request was signed for.
    ///
    /// At most ONE record per Region: the list of delegated administrators a read
    /// reports is `len 0..1`
    /// (`smithy_query(operation="ListOrganizationAdminAccounts")`), so a Region's
    /// organization has one delegated administrator or none. Regional because every
    /// GuardDuty resource is — the designation is made through one Region's
    /// endpoint and only that endpoint reports it
    /// ([`Self::organization_admin_account_in`]).
    ///
    /// BOUNDED at one record per Region across at most
    /// [`MAX_ORGANIZATION_ADMIN_REGIONS`] Regions, which the designation enforces
    /// before it writes ([`Self::organization_admin_regions_exhausted`]), so the
    /// map is finite whatever Region a caller signs for.
    pub organization_admin_accounts: BTreeMap<String, OrganizationAdminAccountRecord>,
    /// Last instant handed out by [`Self::tick_millis`], so two mutations in the
    /// same millisecond still get strictly increasing timestamps.
    last_millis: i64,
    /// Monotonic counter folded into every minted id.
    seq: u64,
}

/// What a member account's request to LEAVE the administrator relationship it
/// accepted an invitation into found, as
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::disassociate_detector_from_administrator`]
/// reports it to the two operations bound to that behaviour.
///
/// Only [`Self::Disassociated`] is a success; each other variant is a refusal with
/// its own message, and none of them writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdministratorDisassociation {
    /// The Region's endpoint holds no detector under that id, so this account
    /// cannot leave anything through it.
    DetectorNotHeld,
    /// The organization this detector's stored configuration describes auto-enables
    /// GuardDuty for every one of its accounts, so the organization — not this call
    /// — owns the relationship (see
    /// [`organization_managed_administrator_message`]).
    OrganizationManaged,
    /// The detector is held, but no OTHER account administers its account — it has
    /// accepted no invitation, or the relationship it had has already ended — so the
    /// account is its own administrator and the request asks it to leave itself (see
    /// [`SELF_DISASSOCIATION_MESSAGE`]).
    AdministratorIsSelf,
    /// This call dropped the relationship.
    Disassociated,
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation, and only once that operation knows
    /// it is going to write: it is the "this resource changed now" clock, so a
    /// reporting operation must not advance it and neither may a call that leaves
    /// the store exactly as it found it — an accepted request that writes nothing
    /// would otherwise push this counter past the wall clock and make the next
    /// real change stamp a future instant. Monotonicity is what makes
    /// `UpdatedAt > CreatedAt` observable for two changes that land in the same
    /// wall-clock millisecond.
    ///
    /// An operation that cannot tell whether it is going to write until it is
    /// writing — a batch that moves only the stored records it finds in a state it
    /// can move — reads [`Self::pending_millis`] and commits it with
    /// [`Self::commit_millis`] instead, which is this same pair of steps with the
    /// write in between.
    pub fn tick_millis(&mut self) -> i64 {
        let next = self.pending_millis();
        self.commit_millis(next);
        next
    }

    /// The instant [`Self::tick_millis`] would hand out, taking NOTHING off the
    /// clock.
    ///
    /// For a mutation whose writes are decided one stored record at a time — a
    /// batch that moves only the records it finds in a state it can move and leaves
    /// every other one exactly as it was — "is this call going to write" is
    /// answerable only while writing. Such a handler reads the instant here, stamps
    /// whatever it moves with it, and takes it off the clock with
    /// [`Self::commit_millis`] once a write has really landed: a call that moved
    /// nothing commits nothing, so it leaves the clock exactly where it found it and
    /// the next real change still stamps a wall-clock instant. A mutation that knows
    /// before it writes whether it is going to reads [`Self::tick_millis`] and
    /// returns ahead of it on the path that writes nothing.
    pub fn pending_millis(&self) -> i64 {
        let wall = now_millis();
        if wall > self.last_millis {
            wall
        } else {
            self.last_millis.saturating_add(1)
        }
    }

    /// Take the instant [`Self::pending_millis`] handed out off the clock, now that
    /// a write carrying it has landed.
    ///
    /// Never moves the clock backwards, so an instant committed twice — and one
    /// committed after a later mutation has already taken a further instant — still
    /// leaves the instants this state hands out strictly increasing.
    pub fn commit_millis(&mut self, millis: i64) {
        self.last_millis = self.last_millis.max(millis);
    }

    /// Mint a fresh 32-hex-character id, the shape GuardDuty uses for both
    /// `DetectorId` and a finding `Id`.
    ///
    /// The low 16 bits carry this state's own sequence counter, so two ids minted
    /// by the same state differ even if the hash above them repeats.
    pub fn mint_id(&mut self) -> String {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let nanos = now_nanos();
        let hi = scramble(seq, nanos);
        let lo = scramble(nanos ^ GOLDEN_RATIO_64, hi ^ seq);
        let lo = (lo & !0xFFFF) | (seq & 0xFFFF);
        format!("{hi:016x}{lo:016x}")
    }

    /// The detector `region`'s endpoint serves for `id`, if any.
    ///
    /// A detector is a REGIONAL resource — "you must create a detector in each
    /// Region where you enable the service"
    /// (`smithy_query(query="doc",shape="CreateDetector")`) — so an id created
    /// through another Region's endpoint is not visible here.
    pub fn detector_in(&self, region: &str, id: &str) -> Option<&Detector> {
        self.detectors
            .get(id)
            .filter(|detector| detector.region == region)
    }

    /// [`Self::detector_in`], for a mutating operation.
    pub fn detector_in_mut(&mut self, region: &str, id: &str) -> Option<&mut Detector> {
        self.detectors
            .get_mut(id)
            .filter(|detector| detector.region == region)
    }

    /// Drop the administrator relationship the account that owns the detector
    /// `region`'s endpoint serves for `id` is monitored by.
    ///
    /// Every outcome a caller has to tell apart is a variant of
    /// [`AdministratorDisassociation`]: only the last of them is a success, and the
    /// two names bound to this behaviour both read it here, so neither can refuse
    /// what the other accepts.
    ///
    /// The ORGANIZATION precondition is checked before the relationship is read,
    /// the way the administrator-side operations check it before they match any
    /// account: it holds for the whole call rather than for one relationship (see
    /// [`organization_managed_administrator_message`]).
    ///
    /// The mutation clock ([`Self::tick_millis`]) is read ONLY on the path that
    /// writes, so no refusal advances it — which is what lets this be one walk in
    /// the handler instead of a resolve-then-tick-then-resolve-again one. The write
    /// itself is [`Detector::disassociate_from_administrator`].
    pub fn disassociate_detector_from_administrator(
        &mut self,
        region: &str,
        id: &str,
    ) -> AdministratorDisassociation {
        let Some(detector) = self.detector_in(region, id) else {
            return AdministratorDisassociation::DetectorNotHeld;
        };
        if detector.organization.auto_enables_all_members() {
            return AdministratorDisassociation::OrganizationManaged;
        }
        if detector.administrator.is_none() {
            return AdministratorDisassociation::AdministratorIsSelf;
        }
        let now = self.tick_millis();
        if self
            .detector_in_mut(region, id)
            .is_some_and(|detector| detector.disassociate_from_administrator(now))
        {
            AdministratorDisassociation::Disassociated
        } else {
            // Unreachable: the relationship was read above under this same borrow of
            // the store. Reported as the absence it would be rather than panicked.
            AdministratorDisassociation::AdministratorIsSelf
        }
    }

    /// The detector `region`'s endpoint minted under `token`, if a create recorded
    /// one and that detector is still held.
    ///
    /// A mapping whose detector was deleted resolves to nothing, so a replayed
    /// create never answers with an id the store no longer holds.
    pub fn detector_for_client_token(&self, region: &str, token: &str) -> Option<&Detector> {
        let record = self
            .client_tokens
            .get(&(region.to_string(), token.to_string()))?;
        self.detector_in(region, &record.detector_id)
    }

    /// Record that `token` minted `detector_id` for `region`, keeping the store
    /// bounded by [`MAX_CLIENT_TOKENS`].
    ///
    /// Two kinds of mapping are dropped to make room, in this order: one whose
    /// detector the store no longer holds (it can never resolve again), then the
    /// OLDEST remaining mapping — the token a caller is least likely to replay.
    /// Evicting a mapping costs a replay its idempotency, never a resource a caller
    /// can still read: the detector itself stays exactly where it was.
    pub fn record_client_token(
        &mut self,
        region: &str,
        token: &str,
        detector_id: &str,
        now: i64,
    ) {
        // Taken out of `self` so the retain below can consult `self.detectors`.
        let mut tokens = std::mem::take(&mut self.client_tokens);
        tokens.retain(|(token_region, _), record| {
            self.detector_in(token_region, &record.detector_id).is_some()
        });
        while tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = tokens
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.recorded_at_millis
                        .cmp(&right.recorded_at_millis)
                        // The instants are strictly increasing, so this only orders
                        // mappings a coarser clock could tie.
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            tokens.remove(&oldest);
        }
        tokens.insert(
            (region.to_string(), token.to_string()),
            ClientTokenRecord {
                detector_id: detector_id.to_string(),
                recorded_at_millis: now,
            },
        );
        self.client_tokens = tokens;
    }

    /// Drop every idempotency mapping naming `detector_id`, so a replayed create
    /// stops resolving to a detector a delete removed.
    pub fn forget_client_tokens_for(&mut self, detector_id: &str) {
        self.client_tokens
            .retain(|_, record| record.detector_id != detector_id);
    }

    /// The Malware Protection plan `region`'s endpoint minted under `token`, if a
    /// create recorded one and that plan is still held.
    ///
    /// Resolved THROUGH [`Self::malware_protection_plan_in`] for the same reason
    /// [`Self::detector_for_client_token`] resolves through [`Self::detector_in`]:
    /// a mapping whose plan was deleted resolves to nothing, so a replayed create
    /// never answers with an id the store no longer holds.
    pub fn malware_protection_plan_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&MalwareProtectionPlan> {
        let record = self
            .malware_protection_plan_client_tokens
            .get(&(region.to_string(), token.to_string()))?;
        self.malware_protection_plan_in(region, &record.plan_id)
    }

    /// Record that `token` minted `plan_id` for `region`, keeping the plan
    /// idempotency index bounded by [`MAX_CLIENT_TOKENS`] — the detector index's
    /// ceiling, for the same reason: the key is request-chosen.
    ///
    /// Two kinds of mapping are dropped to make room, in this order: one whose plan
    /// the store no longer holds (it can never resolve again), then the OLDEST
    /// remaining mapping — the token a caller is least likely to replay. Evicting a
    /// mapping costs a replay its idempotency, never a resource a caller can still
    /// read: the plan itself stays exactly where it was.
    pub fn record_malware_protection_plan_client_token(
        &mut self,
        region: &str,
        token: &str,
        plan_id: &str,
        now: i64,
    ) {
        // Taken out of `self` so the retain below can consult the plan store.
        let mut tokens = std::mem::take(&mut self.malware_protection_plan_client_tokens);
        tokens.retain(|(token_region, _), record| {
            self.malware_protection_plan_in(token_region, &record.plan_id)
                .is_some()
        });
        while tokens.len() >= MAX_CLIENT_TOKENS {
            let oldest = tokens
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.recorded_at_millis
                        .cmp(&right.recorded_at_millis)
                        // The instants are strictly increasing, so this only orders
                        // mappings a coarser clock could tie.
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            tokens.remove(&oldest);
        }
        tokens.insert(
            (region.to_string(), token.to_string()),
            MalwareProtectionPlanTokenRecord {
                plan_id: plan_id.to_string(),
                recorded_at_millis: now,
            },
        );
        self.malware_protection_plan_client_tokens = tokens;
    }

    /// Drop every plan idempotency mapping naming `plan_id`, so a replayed create
    /// stops resolving to a plan a delete removed — the hook a plan delete calls, as
    /// `DeleteDetector` calls [`Self::forget_client_tokens_for`].
    pub fn forget_malware_protection_plan_client_tokens_for(&mut self, plan_id: &str) {
        self.malware_protection_plan_client_tokens
            .retain(|_, record| record.plan_id != plan_id);
    }

    /// Mint an `InvestigationId`: the 32 hexadecimal characters [`Self::mint_id`]
    /// mints, grouped in the 8-4-4-4-12 form.
    ///
    /// `InvestigationId` is `len 1..64, pattern ^[a-fA-F0-9\-]+$`
    /// (`smithy_query(operation="CreateInvestigation")`) — hexadecimal plus
    /// hyphens — so the 36-character grouped form is a value of the member, and it
    /// is distinguishable at a glance from the bare-hex `DetectorId` / finding id.
    pub fn mint_investigation_id(&mut self) -> String {
        let hex = self.mint_id();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Mint a `MalwareProtectionPlanId`: the
    /// [`MALWARE_PROTECTION_PLAN_ID_LEN`] hexadecimal characters that form takes
    /// ([`is_malware_protection_plan_id`]), which is a shorter id than the
    /// [`DETECTOR_ID_LEN`] one [`Self::mint_id`] hands out.
    ///
    /// Taken from the TAIL of a minted id, because that is where this state's own
    /// sequence counter lives: two plan ids minted by the same state differ even if
    /// the hash above them repeats.
    pub fn mint_malware_protection_plan_id(&mut self) -> String {
        let hex = self.mint_id();
        hex[hex.len().saturating_sub(MALWARE_PROTECTION_PLAN_ID_LEN)..].to_string()
    }

    /// Set the archive state of the named findings of the detector `region`'s
    /// endpoint serves for `detector_id`, recording the instant of the write as the
    /// `UpdatedAt` of each finding it moves.
    ///
    /// This is the WHOLE rule both directions of the archive write follow — resolve
    /// the detector, resolve every named finding against it, then write — with the
    /// direction carried by `archived`, so neither direction can drift from the
    /// other.
    ///
    /// `Err` carries the message the request is refused with, already classified by
    /// [`detector_id_rejection`] / [`finding_id_rejection`]: an endpoint that holds
    /// no such detector, or a batch naming a finding id that detector does not hold.
    /// Every id is resolved BEFORE anything is written, and the detector is resolved
    /// before the clock is read at all, so a refused request moves nothing and
    /// advances nothing — and an ACCEPTED request that moves no finding consumes no
    /// instant either ([`Self::pending_millis`]).
    pub fn set_findings_archived_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        archived: bool,
    ) -> Result<(), String> {
        let Some(detector) = self.detector_in(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if let Some(unheld) = detector.first_unheld_finding_id(finding_ids) {
            return Err(finding_id_rejection(unheld));
        }
        // Whether any finding moves is known only as the write walks the named ids —
        // a batch naming none of them, and one naming only findings already in the
        // requested state, each leave every record exactly as it was — so the
        // instant is read here and taken off the clock only once a write has landed.
        let now = self.pending_millis();
        let Some(detector) = self.detector_in_mut(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if detector.set_findings_archived(finding_ids, archived, now) {
            self.commit_millis(now);
        }
        Ok(())
    }

    /// Record the feedback a caller submitted on the named findings of the detector
    /// `region`'s endpoint serves for `detector_id`, recording the instant of the
    /// write as the `UpdatedAt` of each finding it changes.
    ///
    /// This is the WHOLE rule the feedback write follows — resolve the detector,
    /// then write — and it is the same shape the archive write takes
    /// ([`Self::set_findings_archived_in`]), so neither metadata write can drift
    /// from the other on which clock a record-metadata change moves. Unlike the
    /// archive write, an id this detector does not hold is NOT a refusal here: the
    /// feedback write takes the reading a findings read takes, where such an id is
    /// simply absent.
    ///
    /// `Err` carries the message the request is refused with, already classified by
    /// [`detector_id_rejection`]: an endpoint that holds no such detector. The
    /// detector is resolved before the clock is read at all, so a refused request
    /// moves nothing and advances nothing — and an accepted submission that changes
    /// no finding consumes no instant either ([`Self::pending_millis`]).
    pub fn set_findings_feedback_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        feedback: &str,
        comments: Option<&str>,
    ) -> Result<(), String> {
        if self.detector_in(region, detector_id).is_none() {
            return Err(detector_id_rejection(detector_id));
        }
        // Read without being taken off the clock, for the reason
        // [`Self::set_findings_archived_in`] reads it that way: a submission that
        // names no finding this detector holds, and one that re-submits the feedback
        // already recorded, both write nothing.
        let now = self.pending_millis();
        let Some(detector) = self.detector_in_mut(region, detector_id) else {
            return Err(detector_id_rejection(detector_id));
        };
        if detector.set_findings_feedback(finding_ids, feedback, comments, now) {
            self.commit_millis(now);
        }
        Ok(())
    }

    /// The ids of the detectors `region`'s endpoint serves, in a total order.
    pub fn detector_ids_in(&self, region: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .detectors
            .values()
            .filter(|detector| detector.region == region)
            .map(|detector| detector.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The invitations this account's inbox holds through `region`'s endpoint, in
    /// a total order.
    ///
    /// The invitations hang off [`Detector::received_invitations`], and the
    /// operations that report the inbox name no detector at all
    /// (`smithy_query(operation="ListInvitations")` binds `GET /invitation`), so
    /// the inbox a request addresses is the one the Region it was signed for
    /// serves — a detector is a REGIONAL resource, so an invitation delivered to
    /// one Region's endpoint is not in another's inbox.
    ///
    /// Ordered by the inviting account id and then by the invitation id, which is
    /// a TOTAL order over the records: that is what makes a page token mean the
    /// same thing on the call that hands it back.
    ///
    /// One inviting account appears at most once: the key of each inbox IS the
    /// inviting account id, and a Region holds at most one detector ("you can
    /// have only one detector per account per Region", which `CreateDetector`
    /// refuses a second create under), so the walk below cannot see two inboxes
    /// of one account.
    pub fn received_invitations_in(&self, region: &str) -> Vec<&ReceivedInvitationRecord> {
        let mut invitations: Vec<&ReceivedInvitationRecord> = self
            .detectors
            .values()
            .filter(|detector| detector.region == region)
            .flat_map(|detector| detector.received_invitations.values())
            .collect();
        invitations.sort_by(|left, right| {
            left.administrator_account_id
                .cmp(&right.administrator_account_id)
                .then_with(|| left.invitation_id.cmp(&right.invitation_id))
        });
        invitations
    }

    /// Drop the invitation the inbox `region`'s endpoint serves holds from
    /// `administrator_account_id`, reporting whether there was one to drop.
    ///
    /// The inbox is resolved the way [`Self::received_invitations_in`] resolves it
    /// and for the same reason: the operations that answer an invitation by the
    /// account that sent it name no detector, so the inbox a request addresses is
    /// the one the Region it was signed for serves. An account the inbox holds
    /// nothing from is not an error here — the caller reports it as unprocessed —
    /// so this is a lookup that answers `false` and writes NOTHING.
    ///
    /// No instant is read: an invitation carries only the instant it was SENT
    /// ([`ReceivedInvitationRecord::invited_at_millis`], the one member of
    /// `Invitation` that is a time — `smithy_query(shape="Invitation")`), and the
    /// record that held it is gone, so there is nothing left for a clock to
    /// stamp.
    ///
    /// One inbox per Region, for the reason [`Self::received_invitations_in`]
    /// gives: a Region holds at most one detector, so the walk stops at the first
    /// inbox the invitation was in.
    pub fn delete_received_invitation_in(
        &mut self,
        region: &str,
        administrator_account_id: &str,
    ) -> bool {
        self.detectors
            .values_mut()
            .filter(|detector| detector.region == region)
            .find_map(|detector| {
                detector
                    .received_invitations
                    .remove(administrator_account_id)
            })
            .is_some()
    }

    /// Deliver into the inbox `region`'s endpoint serves the invitation an account
    /// that sends one through that endpoint has just sent, reporting whether this
    /// call delivered one.
    ///
    /// The MEMBER side of sending an invitation: "the invited accounts can either
    /// accept or decline the invitation from their GuardDuty accounts"
    /// (`smithy_query(query="doc",shape="InviteMembers")`), so an invitation that
    /// was sent is pending for the account it was delivered to until that account
    /// answers it — which is what [`Detector::accept_invitation`],
    /// [`Self::take_received_invitations_in`] and the inbox reads resolve. The
    /// administrator side of the same send is the member records the caller moves.
    ///
    /// One inbox per Region, resolved the way [`Self::received_invitations_in`]
    /// resolves it: the operations that answer an invitation name no detector, so
    /// the inbox a request addresses is the one the Region it was signed for
    /// serves, and an invitation delivered through one Region's endpoint is not in
    /// another's inbox.
    ///
    /// ONE invitation is pending per inviting account at a time — an acceptance
    /// identifies the invitation by that account and the id alone
    /// (`smithy_query(operation="AcceptInvitation")`) — so an account whose
    /// invitation is still pending KEEPS it: re-sending re-sends that same
    /// invitation rather than issuing a second one, exactly as the member record a
    /// re-send leaves where it was, and no id is minted for it. An account whose
    /// invitation has been accepted, declined or deleted has none pending, so a
    /// later send delivers a fresh one.
    pub fn deliver_invitation_in(
        &mut self,
        region: &str,
        administrator_account_id: &str,
        now: i64,
    ) -> bool {
        // The whole condition the delivery writes under is answered BEFORE anything
        // is minted — the Region's inbox is resolved, and
        // [`Detector::inbox_has_room_for`] answers both of the write's guards — so a
        // call that writes nothing consumes no id: the ids this state hands out are a
        // sequence, and one burned on a path that writes nothing would shift every
        // later id.
        let Some(detector) = self.inbox_in(region) else {
            return false;
        };
        if !detector.inbox_has_room_for(administrator_account_id) {
            return false;
        }
        let detector_id = detector.id.clone();
        let invitation_id = self.mint_id();
        self.detectors
            .get_mut(&detector_id)
            .is_some_and(|detector| {
                detector.deliver_invitation(administrator_account_id, &invitation_id, now)
            })
    }

    /// The inbox `region`'s endpoint serves, if that Region holds a detector at all.
    ///
    /// One inbox per Region, for the reason [`Self::received_invitations_in`] gives:
    /// the operations that deliver an invitation and the operations that answer one
    /// name no detector, and a Region holds at most one detector ("you can have only
    /// one detector per account per Region", which `CreateDetector` refuses a second
    /// create under), so the Region's one detector IS the inbox a request through its
    /// endpoint addresses.
    fn inbox_in(&self, region: &str) -> Option<&Detector> {
        self.detectors
            .values()
            .find(|detector| detector.region == region)
    }

    /// Is the invitation `administrator_account_id` sent through `region`'s endpoint
    /// still pending?
    ///
    /// The ONE question both halves of a send are decided by. Sending an invitation
    /// writes the administrator's own member records AND delivers the invitation to
    /// the inbox the Region's endpoint serves ([`Self::deliver_invitation_in`]), and
    /// an invitation stays pending only until the account it was delivered to answers
    /// it — so taking it out of that inbox, which is what an acceptance
    /// ([`Detector::accept_invitation`]), a decline and a delete
    /// ([`Self::take_received_invitations_in`]) each do, is what makes it no longer
    /// pending for the member records of that same send as much as for the inbox.
    /// Both halves therefore ask HERE, so neither can report an invitation pending
    /// that the other no longer holds.
    pub fn invitation_pending_in(&self, region: &str, administrator_account_id: &str) -> bool {
        self.inbox_in(region)
            .is_some_and(|detector| detector.has_invitation_pending_from(administrator_account_id))
    }

    /// Take out of the inbox `region`'s endpoint serves the invitation each account
    /// in `account_ids` sent, reporting the accounts the inbox held none from as the
    /// `UnprocessedAccounts` the caller answers with.
    ///
    /// The write the two operations that END a pending invitation without accepting
    /// it SHARE: an invited account "can either accept or decline the invitation"
    /// (`smithy_query(query="doc",shape="InviteMembers")`), so declining is an ANSWER
    /// and takes the invitation out of the pending inbox exactly as
    /// [`Detector::accept_invitation`] does, and deleting one discards it from the
    /// same inbox. Both leave the inbox holding nothing from that account, so neither
    /// can be followed by an acceptance, and both report per-account rather than
    /// refusing. They are one write under two names, the way `AcceptInvitation` and
    /// `AcceptAdministratorInvitation` are, so neither operation can drift from the
    /// other on which accounts it acts on or reports.
    ///
    /// `AccountIds` is a SET of accounts: one account's invitation is taken once, so
    /// a repeated id is reported at most once. The reports keep the order the request
    /// named the accounts in, so a caller can read them against its own list.
    ///
    /// Each account resolves through [`Self::delete_received_invitation_in`], which
    /// is where the Region and the writes-nothing-for-a-miss rules live.
    ///
    /// The REASON is the caller's, because the two operations report their own: each
    /// answers an account it could not process with the fixed sentence GuardDuty
    /// defines for that operation's refusal class
    /// ([`INVALID_RELATIONSHIP_TRANSITION_MESSAGE`] for the decline,
    /// [`INVITATION_NOT_DELETED_MESSAGE`] for the discard), so sharing the write does
    /// not make them share a wording.
    pub fn take_received_invitations_in(
        &mut self,
        region: &str,
        account_ids: &[String],
        result: &str,
    ) -> Vec<crate::ops::types::shared_types::UnprocessedAccount> {
        let mut unprocessed = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for account_id in account_ids {
            if !seen.insert(account_id.as_str()) {
                continue;
            }
            if !self.delete_received_invitation_in(region, account_id) {
                // Nothing to take: this account has sent the current one no
                // invitation, or the one it sent has already been answered or
                // deleted. Reported, and nothing is persisted for it.
                unprocessed.push(crate::ops::types::shared_types::UnprocessedAccount {
                    account_id: account_id.clone(),
                    result: result.to_string(),
                });
            }
        }
        unprocessed
    }

    /// The Malware Protection plan `region`'s endpoint serves for `id`, if any.
    ///
    /// Regional for the same reason a detector is: a plan is created through one
    /// Region's endpoint and only that endpoint reports it.
    pub fn malware_protection_plan_in(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&MalwareProtectionPlan> {
        self.malware_protection_plans
            .get(id)
            .filter(|plan| plan.region == region)
    }

    /// The plan `region` already holds for the S3 bucket named `bucket_name`, if
    /// any — the plan a second create for that same protected resource conflicts
    /// with.
    pub fn malware_protection_plan_for_bucket(
        &self,
        region: &str,
        bucket_name: &str,
    ) -> Option<&MalwareProtectionPlan> {
        self.malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .find(|plan| plan.bucket_name == bucket_name)
    }

    /// [`Self::malware_protection_plan_for_bucket`], for a caller that writes to the
    /// plan it resolves — `SendObjectMalwareScan` recording the object scan it
    /// accepted against the plan protecting the object's bucket.
    pub fn malware_protection_plan_for_bucket_mut(
        &mut self,
        region: &str,
        bucket_name: &str,
    ) -> Option<&mut MalwareProtectionPlan> {
        self.malware_protection_plans
            .values_mut()
            .filter(|plan| plan.region == region)
            .find(|plan| plan.bucket_name == bucket_name)
    }

    /// How many Malware Protection plans `region`'s endpoint holds — the count the
    /// per-account-per-Region quota
    /// ([`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`]) is measured
    /// against.
    pub fn malware_protection_plan_count_in(&self, region: &str) -> usize {
        self.malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .count()
    }

    /// The ids of the Malware Protection plans `region`'s endpoint serves, in a
    /// total order.
    ///
    /// The listing counterpart of [`Self::malware_protection_plan_in`], regional for
    /// the same reason, and the same set [`Self::malware_protection_plan_count_in`]
    /// counts — so a plan a delete removed stops being listed. The order is total,
    /// which is what makes a page token mean the same entry on the call that hands
    /// it back.
    pub fn malware_protection_plan_ids_in(&self, region: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .malware_protection_plans
            .values()
            .filter(|plan| plan.region == region)
            .map(|plan| plan.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Would a detector created through `region`'s endpoint partition the detector
    /// store into one Region more than it will hold
    /// ([`MAX_DETECTOR_REGIONS`])?
    ///
    /// "One detector per account per Region" bounds each PARTITION to a single
    /// detector, but the Region is read from the request's credential scope
    /// ([`request_region`]), so the caller chooses the partition KEY too: without
    /// this, a caller signing for invented Region names could make the store retain
    /// one detector — and everything it owns — per distinct Region string. A Region
    /// the store already holds a detector for is never refused by it, so this bound
    /// is invisible to a caller using the Regions AWS runs.
    pub fn detector_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for detector in self.detectors.values() {
            regions.insert(detector.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_DETECTOR_REGIONS
    }

    /// Would a plan created through `region`'s endpoint partition this store into
    /// one Region more than it will hold
    /// ([`MAX_MALWARE_PROTECTION_PLAN_REGIONS`])?
    ///
    /// The Region is request-chosen, so this is the bound on the number of
    /// PARTITIONS that makes the per-Region plan quota a bound on the whole plan
    /// map. A Region the store already holds a plan for is never refused by it.
    pub fn malware_protection_plan_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for plan in self.malware_protection_plans.values() {
            regions.insert(plan.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_MALWARE_PROTECTION_PLAN_REGIONS
    }

    /// The malware scan `region`'s endpoint serves for `scan_id`, if any.
    ///
    /// Regional for the same reason a detector and a plan are: a scan is started
    /// through one Region's endpoint and only that endpoint reports it.
    pub fn malware_scan_in(&self, region: &str, scan_id: &str) -> Option<&MalwareScan> {
        self.malware_scans
            .get(scan_id)
            .filter(|scan| scan.region == region)
    }

    /// [`Self::malware_scan_in`], for a caller that materialises the status change
    /// the clock already owes the scan ([`MalwareScan::settle`]).
    pub fn malware_scan_in_mut(&mut self, region: &str, scan_id: &str) -> Option<&mut MalwareScan> {
        self.malware_scans
            .get_mut(scan_id)
            .filter(|scan| scan.region == region)
    }

    /// Every malware scan `region`'s endpoint serves, in scan-id order — a total
    /// order, so a page token stays meaningful before any sort is applied.
    ///
    /// The listing counterpart of [`Self::malware_scan_in`], and regional for the
    /// same reason: a scan started through another Region's endpoint is not served
    /// here.
    pub fn malware_scans_in(&self, region: &str) -> Vec<&MalwareScan> {
        let mut scans: Vec<&MalwareScan> = self
            .malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .collect();
        scans.sort_by(|a, b| a.id.cmp(&b.id));
        scans
    }

    /// Materialise the status change the CLOCK already owes EVERY scan this store
    /// holds, through [`MalwareScan::settle`].
    ///
    /// The per-scan counterpart is what `GetMalwareScan` applies to the one scan it
    /// reports; a listing reports a set, so it owes the same move to each of them.
    /// Each was owed at that scan's own `terminal_due_millis` whether or not
    /// anybody called, which is why a reporting operation may apply it.
    pub fn settle_malware_scans(&mut self) {
        let now = now_millis();
        for scan in self.malware_scans.values_mut() {
            scan.settle(now);
        }
    }

    /// The scan `region`'s endpoint started under `token`, if a start recorded one.
    ///
    /// The token is a pure REPLAY key: it resolves to the scan the first start
    /// minted and nothing about the replaying request is compared against it, so a
    /// replay answers with that scan rather than starting a second one.
    pub fn malware_scan_for_client_token(&self, region: &str, token: &str) -> Option<&MalwareScan> {
        self.malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .find(|scan| scan.client_token.as_deref() == Some(token))
    }

    /// How many malware scans `region`'s endpoint holds — the count
    /// [`MAX_MALWARE_SCANS_PER_REGION`] is measured against.
    pub fn malware_scan_count_in(&self, region: &str) -> usize {
        self.malware_scans
            .values()
            .filter(|scan| scan.region == region)
            .count()
    }

    /// Would a scan started through `region`'s endpoint partition the scan store
    /// into one Region more than it will hold ([`MAX_MALWARE_SCAN_REGIONS`])?
    ///
    /// The Region is request-chosen, so this is the bound on the number of
    /// PARTITIONS that makes the per-Region scan ceiling a bound on the whole scan
    /// map. A Region the store already holds a scan for is never refused by it.
    pub fn malware_scan_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for scan in self.malware_scans.values() {
            regions.insert(scan.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_MALWARE_SCAN_REGIONS
    }

    /// The association `region`'s endpoint holds for `rule_id`, if any.
    ///
    /// A rule carries at most ONE association per account, whatever mode it is in,
    /// so this is both the duplicate check a create makes and the way the rule's
    /// association is reached. Regional for the same reason a detector is: an
    /// association is made through one Region's endpoint and only that endpoint
    /// reports it.
    pub fn detection_rule_association(
        &self,
        region: &str,
        rule_id: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .get(&(region.to_string(), rule_id.to_string()))
    }

    /// [`Self::detection_rule_association`], for an operation that is going to
    /// change the association it finds.
    pub fn detection_rule_association_mut(
        &mut self,
        region: &str,
        rule_id: &str,
    ) -> Option<&mut DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .get_mut(&(region.to_string(), rule_id.to_string()))
    }

    /// The association `region`'s endpoint holds under `association_id` for
    /// `rule_id`, if any.
    ///
    /// The id is a handle onto the rule's association, so this resolves it by
    /// checking the entry that rule's key names: an id naming no association of
    /// `rule_id` resolves to nothing, exactly as one naming no association at all
    /// does.
    pub fn detection_rule_association_by_id(
        &self,
        region: &str,
        rule_id: &str,
        association_id: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.detection_rule_association(region, rule_id)
            .filter(|record| record.association_id == association_id)
    }

    /// The association `region`'s endpoint made under `token`, if a create
    /// recorded one.
    ///
    /// The token is a pure REPLAY key, as it is for every other create on this
    /// service: it resolves to the association the first call made and nothing
    /// about the replaying request is compared against it, so a replay answers
    /// with that association rather than making a second one or mutating the
    /// first.
    pub fn detection_rule_association_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&DetectionRuleAssociationRecord> {
        self.custom_detection_rule_associations
            .values()
            .filter(|record| record.region == region)
            .find(|record| record.client_token.as_deref() == Some(token))
    }

    /// The organization configuration `region`'s endpoint holds for
    /// `(rule_id, mode)`, if any.
    pub fn detection_rule_org_configuration(
        &self,
        region: &str,
        rule_id: &str,
        mode: &str,
    ) -> Option<&DetectionRuleOrgConfigurationRecord> {
        self.custom_detection_rule_org_configurations.get(&(
            region.to_string(),
            rule_id.to_string(),
            mode.to_string(),
        ))
    }

    /// The `AssociationMode`s `region`'s endpoint holds an organization
    /// configuration for `rule_id` in, in mode order.
    ///
    /// `UpdateCustomDetectionRuleOrgConfiguration` identifies its configuration by
    /// `RuleId` ALONE — the operation binds `RuleId` as the URI label and `Mode` to
    /// the body, where the read and the delete bind `Mode` as a query parameter
    /// (`smithy_query(operation="UpdateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration")`)
    /// — so it needs the modes a rule is configured in rather than one
    /// `(rule_id, mode)` lookup. A Region holds at most one configuration per
    /// published rule per mode, so this scans a table of fixed,
    /// single-digit-times-two length, exactly as
    /// [`Self::detection_rule_association_by_id`] does.
    pub fn detection_rule_org_configuration_modes(
        &self,
        region: &str,
        rule_id: &str,
    ) -> Vec<String> {
        self.custom_detection_rule_org_configurations
            .values()
            .filter(|record| record.region == region && record.rule_id == rule_id)
            .map(|record| record.mode.clone())
            .collect()
    }

    /// The organization configuration `region`'s endpoint created under `token`,
    /// if a create recorded one. A pure REPLAY key, exactly as
    /// [`Self::detection_rule_association_for_client_token`] is.
    pub fn detection_rule_org_configuration_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&DetectionRuleOrgConfigurationRecord> {
        self.custom_detection_rule_org_configurations
            .values()
            .filter(|record| record.region == region)
            .find(|record| record.client_token.as_deref() == Some(token))
    }

    /// Would a custom-detection-rule write through `region`'s endpoint partition
    /// the association and organization-configuration stores into one Region more
    /// than they will hold ([`MAX_DETECTION_RULE_REGIONS`])?
    ///
    /// The Region is request-chosen ([`request_region`]), so this is the bound on
    /// the number of PARTITIONS that makes the per-Region ceiling — one association
    /// per published rule, and one organization configuration per published rule
    /// per `AssociationMode` member — a bound on the whole store. A
    /// Region the store already holds a record for is never refused by it, so the
    /// bound is invisible to a caller using the Regions AWS runs. Counted over
    /// BOTH maps, because both are keyed by the same request-chosen Region.
    pub fn detection_rule_regions_exhausted(&self, region: &str) -> bool {
        let mut regions: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for record in self.custom_detection_rule_associations.values() {
            regions.insert(record.region.as_str());
        }
        for record in self.custom_detection_rule_org_configurations.values() {
            regions.insert(record.region.as_str());
        }
        !regions.contains(region) && regions.len() >= MAX_DETECTION_RULE_REGIONS
    }

    /// The delegated administrator `region`'s endpoint holds, if any.
    ///
    /// A designation is made through one Region's endpoint, so a designation made
    /// through another Region's is not visible here — the same rule
    /// [`Self::detector_in`] applies to a detector.
    pub fn organization_admin_account_in(
        &self,
        region: &str,
    ) -> Option<&OrganizationAdminAccountRecord> {
        self.organization_admin_accounts.get(region)
    }

    /// Designate `admin_account_id` as the delegated administrator `region`'s
    /// endpoint serves, replacing nothing: the caller has already established that
    /// the Region holds no administrator.
    pub fn designate_organization_admin_account(&mut self, region: &str, admin_account_id: &str) {
        self.organization_admin_accounts.insert(
            region.to_string(),
            OrganizationAdminAccountRecord {
                admin_account_id: admin_account_id.to_string(),
                disable_due_millis: None,
            },
        );
    }

    /// Accept the removal of the delegated administrator `region`'s endpoint holds,
    /// through [`OrganizationAdminAccountRecord::begin_disable`].
    ///
    /// `None` when that endpoint holds no delegated administrator at all;
    /// `Some(false)` when the one it holds is a different account than
    /// `admin_account_id`, which is not a removal this call may make; `Some(true)`
    /// when the removal was accepted, whether this call is the one that accepted it
    /// or a repeat of one already in flight. The mutation clock is read only on the
    /// path that writes, so neither refusal advances it and neither does that
    /// repeat — the discipline
    /// [`Self::disassociate_detector_from_administrator`] keeps too.
    pub fn disable_organization_admin_account(
        &mut self,
        region: &str,
        admin_account_id: &str,
    ) -> Option<bool> {
        let record = self.organization_admin_account_in(region)?;
        if record.admin_account_id != admin_account_id {
            return Some(false);
        }
        // Asking again for a removal still in flight asks for the state the record is
        // already transitioning to: the instant the first request made due stands
        // ([`OrganizationAdminAccountRecord::begin_disable`]), so such a call is
        // accepted and writes nothing at all. Tested BEFORE the clock is read, exactly
        // as [`Self::disassociate_detector_from_administrator`] tests the relationship
        // it drops, so a repeat consumes no instant of the "this resource changed now"
        // counter either.
        if record.disable_due_millis.is_some() {
            return Some(true);
        }
        let now = self.tick_millis();
        if let Some(record) = self.organization_admin_accounts.get_mut(region) {
            record.begin_disable(now);
        }
        Some(true)
    }

    /// Turn around the removal still in flight for `admin_account_id`, the
    /// delegated administrator `region`'s endpoint holds, through
    /// [`OrganizationAdminAccountRecord::cancel_disable`].
    ///
    /// `true` when a pending removal was turned around, `false` when that
    /// endpoint holds no administrator, holds a DIFFERENT one — a designation of
    /// another account is a second administrator rather than a reversal, so this
    /// never replaces the account the record holds — or holds this one with no
    /// removal pending, in which case nothing was owed and nothing changed,
    /// exactly as a repeated request leaves [`EntitySetRecord::set_activation`]
    /// where it is. The account is checked HERE, where the record is reached, so
    /// the reversal cannot be applied to a record the request does not name.
    pub fn redesignate_organization_admin_account(
        &mut self,
        region: &str,
        admin_account_id: &str,
    ) -> bool {
        self.organization_admin_accounts
            .get_mut(region)
            .is_some_and(|record| {
                record.admin_account_id == admin_account_id && record.cancel_disable()
            })
    }

    /// Materialise the removal the CLOCK already owes every delegated
    /// administrator whose disable has fallen due.
    ///
    /// Each removal was owed at that record's own
    /// [`OrganizationAdminAccountRecord::disable_due_millis`] whether or not
    /// anybody called, which is why a reporting operation may apply it — the same
    /// reason [`Self::settle_malware_scans`] may.
    pub fn settle_organization_admin_accounts(&mut self) {
        let now = now_millis();
        self.organization_admin_accounts
            .retain(|_, record| !record.disable_settled(now));
    }

    /// Would a designation through `region`'s endpoint partition this store into
    /// one Region more than it will hold ([`MAX_ORGANIZATION_ADMIN_REGIONS`])?
    ///
    /// The Region is request-chosen ([`request_region`]), so the per-Region ceiling
    /// of one record needs a ceiling on the number of PARTITIONS to bound the whole
    /// map — exactly as [`Self::detection_rule_regions_exhausted`] does for the
    /// rule stores. A Region the store already holds a record for is never refused
    /// by it.
    pub fn organization_admin_regions_exhausted(&self, region: &str) -> bool {
        !self.organization_admin_accounts.contains_key(region)
            && self.organization_admin_accounts.len() >= MAX_ORGANIZATION_ADMIN_REGIONS
    }
}

/// The most `(region, ClientToken)` -> detector mappings the store retains. The
/// plan idempotency index
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::malware_protection_plan_client_tokens`])
/// is bounded by the same number, for the same reason.
///
/// A STORE GUARD, not a modelled quota: AWS publishes no limit on how many
/// idempotency tokens an account may use, and nothing a caller can read reports this
/// number. It exists because the key is request-chosen, so the map needs a ceiling
/// the way [`MAX_MEMBERS_PER_DETECTOR`] gives the request-keyed member store one.
/// Past it, [`ComAmazonawsGuarddutyGuarddutyapiserviceState::record_client_token`]
/// evicts the oldest mapping.
pub const MAX_CLIENT_TOKENS: usize = 1_024;

/// One idempotency mapping: the detector a `ClientToken` minted, and when the create
/// that minted it was recorded.
///
/// The instant is what makes eviction ORDERED rather than arbitrary: the oldest
/// mapping goes first, so the token a caller just used is the last to be dropped.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The `DetectorId` the create minted.
    pub detector_id: String,
    /// When the mapping was recorded, in epoch milliseconds from
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`].
    pub recorded_at_millis: i64,
}

/// [`ClientTokenRecord`] for the Malware Protection plan index: the plan a
/// `ClientToken` minted, and when the create that minted it was recorded.
///
/// A separate record rather than a reused one because the two indexes resolve
/// different resource types, and a token must never hand back the other store's id.
#[derive(Debug, Clone)]
pub struct MalwareProtectionPlanTokenRecord {
    /// The `MalwareProtectionPlanId` the create minted.
    pub plan_id: String,
    /// When the mapping was recorded, in epoch milliseconds from
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`].
    pub recorded_at_millis: i64,
}

/// The 64-bit golden-ratio odd constant used to decorrelate the two hash inputs.
const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;

/// Hash two words into one, seeded by a freshly built [`std::collections::hash_map::RandomState`]
/// so ids are not predictable across runs.
fn scramble(a: u64, b: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(a);
    hasher.write_u64(b);
    hasher.finish()
}

/// The current instant in epoch milliseconds, straight off the wall clock.
///
/// A REPORTING operation reads the clock through this rather than through
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`]: it needs to know
/// what the clock has already made due without advancing the "this resource
/// changed now" counter.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
}

/// Render an epoch-millisecond instant the way GuardDuty renders `CreatedAt` /
/// `UpdatedAt`: an ISO-8601 instant in UTC with a fixed three-digit millisecond
/// field.
///
/// The width is fixed deliberately. RFC 3339 allows trimming trailing zeros from
/// the subsecond field, but then `…:21.62Z` and `…:21.621Z` no longer compare in
/// time order as text, and `CreatedAt`/`UpdatedAt` are modelled as `String` — text
/// is all a client has to order them by.
pub fn iso8601(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let subsec_millis = millis.rem_euclid(1000);
    let whole = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    match whole.strip_suffix('Z') {
        Some(prefix) => format!("{prefix}.{subsec_millis:03}Z"),
        None => whole,
    }
}

/// One instant of an SDK timestamp in epoch milliseconds, the form this service holds
/// every instant in — the ONE conversion every instant, reported or received, goes
/// through.
///
/// It SATURATES at the representable bound rather than failing, because the bound is
/// this store's and not the API's: a modelled `Timestamp` carries no `@range`, so an
/// instant whose milliseconds do not fit an `i64` is input the service must accept.
/// Saturating keeps the instant — and so the member that carries it — present and in
/// time order; a fallible conversion would turn a value the caller sent into no value
/// at all.
pub fn millis_of(instant: &::aws_smithy_types::DateTime) -> i64 {
    instant
        .secs()
        .saturating_mul(1_000)
        .saturating_add(i64::from(instant.subsec_nanos() / 1_000_000))
}

/// `DetectorStatus` / `FeatureStatus` / `DataSourceStatus` — every one of the three
/// enums is `ENABLED | DISABLED` (`smithy_query(shape="DetectorStatus")`).
pub const ENABLED: &str = "ENABLED";
/// See [`ENABLED`].
pub const DISABLED: &str = "DISABLED";

/// Every member of the `DetectorFeature` enum, in model order
/// (`smithy_query(shape="DetectorFeature")`). These are the features a request may
/// configure: `DetectorFeatureConfiguration.Name` targets this enum.
pub const DETECTOR_FEATURES: [&str; 9] = [
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
    "AI_ANALYST",
];

/// Runtime Monitoring: the feature `CreateDetector` documents as the exception to
/// its "all the optional features are enabled by default" default
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_RUNTIME_MONITORING: &str = "RUNTIME_MONITORING";

/// Runtime Monitoring already covers Amazon EKS, so the two cannot both be
/// requested: "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and
/// Runtime Monitoring (RUNTIME_MONITORING) will cause an error"
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_EKS_RUNTIME_MONITORING: &str = "EKS_RUNTIME_MONITORING";

/// The feature that governs S3 data-event ingestion — the `DataSources.S3Logs`
/// view of the same switch.
pub const FEATURE_S3_DATA_EVENTS: &str = "S3_DATA_EVENTS";
/// The feature behind `DataSources.Kubernetes.AuditLogs`.
pub const FEATURE_EKS_AUDIT_LOGS: &str = "EKS_AUDIT_LOGS";
/// The feature behind `DataSources.MalwareProtection.ScanEc2InstanceWithFindings`.
pub const FEATURE_EBS_MALWARE_PROTECTION: &str = "EBS_MALWARE_PROTECTION";
/// The feature behind `DataSources.Database.Rds.DatabaseEvents`: RDS Protection
/// watches Amazon RDS login activity.
pub const FEATURE_RDS_LOGIN_EVENTS: &str = "RDS_LOGIN_EVENTS";

/// GuardDuty Investigation's own feature: "To use this operation, the AI_ANALYST
/// feature must be enabled on your detector"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const FEATURE_AI_ANALYST: &str = "AI_ANALYST";

/// The feature behind `DataSources.CloudTrail`: GuardDuty's analysis of CloudTrail
/// management events.
pub const FEATURE_CLOUD_TRAIL: &str = "CLOUD_TRAIL";
/// The feature behind `DataSources.DNSLogs`.
pub const FEATURE_DNS_LOGS: &str = "DNS_LOGS";
/// The feature behind `DataSources.FlowLogs`.
pub const FEATURE_FLOW_LOGS: &str = "FLOW_LOGS";

/// The `EbsSnapshotPreservation` member that retains nothing: Malware Protection
/// deletes the EBS volume snapshots it took for a scan once the scan finishes, which
/// is the setting a detector has until a request asks for the other member
/// (retention when the scan produced a malware finding).
///
/// Named, not taken by position out of the generated enum list: the const assertion
/// below fails the BUILD if the model stops carrying this member, where
/// `EBS_SNAPSHOT_PRESERVATION_VALUES[0]` would instead have silently made some other
/// member the detector's default. That list is also what the request boundary
/// refuses a value outside the enum against, before any handler runs.
pub const NO_RETENTION: &str = "NO_RETENTION";

const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::EBS_SNAPSHOT_PRESERVATION_VALUES,
    NO_RETENTION
));

/// Does the generated enum list `values` carry `value`?
///
/// A `const fn` over bytes because `==` on `str` is not usable in a const assertion;
/// it exists only for compile-time checks like the one above — a handler that names a
/// modelled enum member in a constant asserts it the same way, so the BUILD fails if
/// the model stops carrying that member.
pub const fn enum_carries(values: &[&str], value: &str) -> bool {
    let mut index = 0;
    while index < values.len() {
        if bytes_equal(values[index].as_bytes(), value.as_bytes()) {
            return true;
        }
        index += 1;
    }
    false
}

/// Byte-wise equality, for [`enum_carries`]'s const context.
const fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// The reserved tag-key prefix no `ScanConditionPair.Key` may use, as the member's
/// own pattern states it — `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// (`smithy_query(shape="ScanConditionPair")`).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The optional features a bare `CreateDetector` leaves DISABLED — the ones a
/// detector reports only once a request asks for them.
///
/// Runtime Monitoring is the ONE exception `CreateDetector` documents: "When you
/// don't specify any features, with an exception to RUNTIME_MONITORING, all the
/// optional features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). Every other configurable
/// feature — RDS Protection included — is therefore enabled on a detector created
/// without a `Features` list, and reports so under both views of its switch.
pub const OPT_IN_FEATURES: [&str; 1] = [FEATURE_RUNTIME_MONITORING];

/// The three data sources GuardDuty always consumes for a detector — CloudTrail
/// management events, VPC flow logs and DNS logs.
///
/// They are reported as features (`DetectorFeatureResult` carries `CLOUD_TRAIL`,
/// `DNS_LOGS` and `FLOW_LOGS`, `smithy_query(shape="DetectorFeatureResult")`) but
/// are NOT members of `DetectorFeature` and have no `DataSourceConfigurations`
/// member, so no request can switch them off: they report `ENABLED` for the life of
/// the detector.
pub const FOUNDATIONAL_FEATURES: [&str; 3] =
    [FEATURE_CLOUD_TRAIL, FEATURE_DNS_LOGS, FEATURE_FLOW_LOGS];

/// Every feature a detector reports, in the order the service reports them: the
/// three foundational sources first, in the order `DataSourceConfigurationsResult`
/// lists them (`CloudTrail`, `DNSLogs`, `FlowLogs`), then every configurable
/// [`DETECTOR_FEATURES`] member in `DetectorFeature` model order.
///
/// The list is a total order over a SET, so a client may index it positionally.
pub fn reported_features() -> Vec<&'static str> {
    FOUNDATIONAL_FEATURES
        .iter()
        .chain(DETECTOR_FEATURES.iter())
        .copied()
        .collect()
}

/// The current GuardDuty finding schema version, the value every finding carries
/// in its required `SchemaVersion` member.
pub const FINDING_SCHEMA_VERSION: &str = "2.0";

/// The partition every emulated ARN lives in (`engine::emulator_arn` builds
/// `arn:aws:...`).
pub const PARTITION: &str = "aws";

/// The only member of the `DestinationType` enum: "Currently only Amazon S3
/// buckets are supported" (`smithy_query(shape="DestinationType")` lists `S3`).
pub const DESTINATION_TYPE_S3: &str = "S3";

/// Every member of `DestinationType`, in model order
/// (`smithy_query(shape="DestinationType")`).
pub const DESTINATION_TYPES: [&str; 1] = [DESTINATION_TYPE_S3];

/// A freshly created publishing destination's `PublishingStatus`: GuardDuty has
/// not yet verified that it can write to the destination
/// (`smithy_query(shape="PublishingStatus")`).
pub const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";

/// A verified destination's `PublishingStatus`: GuardDuty is exporting findings
/// to it (`smithy_query(shape="PublishingStatus")`).
pub const PUBLISHING: &str = "PUBLISHING";

/// Every member of `PublishingStatus`, in model order
/// (`smithy_query(shape="PublishingStatus")`).
pub const PUBLISHING_STATUSES: [&str; 4] = [
    PENDING_VERIFICATION,
    PUBLISHING,
    "UNABLE_TO_PUBLISH_FIX_DESTINATION_PROPERTY",
    "STOPPED",
];

/// How long after `CreatePublishingDestination` the
/// `PENDING_VERIFICATION` -> [`PUBLISHING`] transition comes due.
///
/// A new destination is returned in a non-terminal status, so the move to
/// [`PUBLISHING`] is a transition the CLOCK owes. AWS documents no span for the
/// verification, so this one is chosen: non-zero, so a caller reading straight
/// back still observes `PENDING_VERIFICATION` (and whatever that status refuses),
/// and short enough that nothing has to wait long for the settled status.
pub const VERIFICATION_DELAY_MILLIS: i64 = 500;

/// How many publishing destinations one detector may hold.
///
/// `ListPublishingDestinations` reports a detector's destinations in pages of
/// `MaxResults` `range 1..50`
/// (`smithy_query(operation="ListPublishingDestinations")`), so a detector's whole
/// collection fits in one page of at most 50. Neither the model nor this
/// operation's documentation carries a destinations-per-detector figure, so — like
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] — this is an
/// EMULATOR-CHOSEN bound rather than a published quota: a 51st destination is
/// refused here, where the real service may accept it.
///
/// The same number BOUNDS the store: a destination is retained until its detector
/// is deleted and has no expiry of its own, and it carries the `ClientToken` that
/// minted it, so without this maximum a caller varying or omitting `ClientToken`
/// could make the service retain records — and token mappings — without limit.
pub const PUBLISHING_DESTINATIONS_PER_DETECTOR: usize = 50;

/// The message a create past [`PUBLISHING_DESTINATIONS_PER_DETECTOR`] is refused
/// with. Like every other rejection here it names the resource class and the
/// limit, never the caller's values.
pub fn publishing_destination_limit_message() -> String {
    format!(
        "The request is rejected because the detector has reached the limit of {PUBLISHING_DESTINATIONS_PER_DETECTOR} publishing destinations."
    )
}

/// Is `arn` a well-formed ARN of `service`?
///
/// Form only — nothing resolves the resource it names. An ARN is
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` with a non-empty
/// partition and resource; `region` and `account-id` are empty for the S3 bucket
/// ARNs GuardDuty publishes to (`arn:aws:s3:::DOC-EXAMPLE-BUCKET/myFolder/` —
/// `smithy_query(query="doc",shape="DestinationProperties",member="DestinationArn")`),
/// and the resource keeps any further `:` it carries.
pub fn is_arn_for(service: &str, arn: &str) -> bool {
    arn_fields(arn).is_some_and(|[_, partition, arn_service, _, _, resource]| {
        !partition.is_empty() && arn_service == service && !resource.is_empty()
    })
}

/// The six fields of `arn` — `arn`, partition, service, region, account id and
/// resource — or `None` unless it has all six and opens with `arn`.
///
/// The ONE definition of how an ARN splits in this crate: [`is_arn_for`] judges
/// the fields and [`parse_tagged_resource_arn`] reads them, so the two cannot
/// disagree about what a well-formed ARN is. The resource keeps any further `:`
/// it carries.
fn arn_fields(arn: &str) -> Option<[&str; 6]> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" {
        return None;
    }
    Some([prefix, partition, service, region, account_id, resource])
}

// ---------------------------------------------------------------------------
// Client rejections
//
// GuardDuty classifies a rejection by the REQUEST, not by what the store holds,
// and no message quotes the caller's value or the endpoint's Region back at
// them. Which SENTENCE it answers with belongs to the resource family the
// identifier addresses, and to the class of operation that was asked:
//
//   * the DETECTOR family names the member at fault — a value that is not a
//     well-formed detector id is a parameter-validation failure, a well-formed
//     one this account/Region does not hold an ownership failure;
//   * every family rooted IN a detector answers its own fixed absent-resource
//     sentence instead, which names no member at all and makes no claim about
//     the account ([`NO_SUCH_RESOURCE_MESSAGE`] and the family constants beside
//     it).
//
// The messages are built here, once, so every operation of a family rejects the
// same input the same way.
// ---------------------------------------------------------------------------

/// The wire name of the `DetectorId` member, as the request carries it
/// (`smithy_query(operation="GetDetector")` renders `DetectorId` with the
/// `jsonName`/URI label `detectorId`). It is what a rejection names.
pub const DETECTOR_ID_MEMBER: &str = "detectorId";

/// The wire name of the `NextToken` member every paginated operation here carries
/// (`smithy_query(operation="ListDetectors")`), for the same reason.
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The wire name of the `DestinationId` member
/// (`smithy_query(operation="DescribePublishingDestination")` renders it as the
/// `destinationId` JSON name / URI label). The family's own absence refusals name no
/// member ([`DESTINATION_NOT_FOUND_MESSAGE`], [`DESTINATION_ABSENT_MESSAGE`]).
pub const DESTINATION_ID_MEMBER: &str = "destinationId";

/// The wire path of `DestinationProperties.DestinationArn`, as a rejection names
/// it (`smithy_query(shape="DestinationProperties")` gives the members the JSON
/// names `destinationArn`, `kmsKeyArn` and `expectedBucketOwner`).
pub const DESTINATION_ARN_MEMBER: &str = "destinationProperties.destinationArn";

/// The wire path of `DestinationProperties.KmsKeyArn`. See
/// [`DESTINATION_ARN_MEMBER`].
pub const KMS_KEY_ARN_MEMBER: &str = "destinationProperties.kmsKeyArn";

/// The wire name of the `DestinationType` member
/// (`smithy_query(operation="CreatePublishingDestination")`).
pub const DESTINATION_TYPE_MEMBER: &str = "destinationType";

/// The wire path of `DestinationProperties.ExpectedBucketOwner`. See
/// [`DESTINATION_ARN_MEMBER`].
pub const EXPECTED_BUCKET_OWNER_MEMBER: &str = "destinationProperties.expectedBucketOwner";

/// The wire name of the TOP-LEVEL `ExpectedBucketOwner` member the IPSet, threat
/// list and entity set creates and updates carry
/// (`smithy_query(operation="CreateIPSet")` renders it `expectedBucketOwner`), as
/// distinct from the one nested in `DestinationProperties`
/// ([`EXPECTED_BUCKET_OWNER_MEMBER`]): the same fact, named at a different place in
/// the request.
pub const LIST_EXPECTED_BUCKET_OWNER_MEMBER: &str = "expectedBucketOwner";

/// The wire name of the `ThreatIntelSetId` member, as the request carries it in the
/// URI (`smithy_query(operation="GetThreatIntelSet")` renders it
/// `threatIntelSetId`). The family's own absence refusal names no member
/// ([`SET_ID_NOT_FOUND_MESSAGE`]).
pub const THREAT_INTEL_SET_ID_MEMBER: &str = "threatIntelSetId";

/// The name GuardDuty gives the threat-list `Location` member in a rejection of
/// it: the threat-list counterpart of [`IP_SET_LOCATION_MEMBER`], qualified by
/// its own resource the way [`THREAT_INTEL_SET_ID_MEMBER`] is.
pub const THREAT_INTEL_SET_LOCATION_MEMBER: &str = "threatIntelSetLocation";

/// The wire name of the `ScanResourceCriteria` member of
/// `UpdateMalwareScanSettings` (`jsonName` `scanResourceCriteria`), for the same
/// reason.
pub const SCAN_RESOURCE_CRITERIA_MEMBER: &str = "scanResourceCriteria";

/// The wire name of the account-id member every members operation carries:
/// `AccountIds`' entries target `AccountId`
/// (`smithy_query(operation="DisassociateMembers")`), and `AccountDetail.AccountId`
/// renders as `accountId` (`smithy_query(shape="AccountDetail")`).
pub const ACCOUNT_ID_MEMBER: &str = "accountId";

/// The wire name of the `AccountIds` list every batch members operation carries the
/// account ids in (`smithy_query(operation="DisassociateMembers")` renders it
/// `accountIds`), including the one `GetUsageStatistics` nests inside its criteria
/// (`smithy_query(query="shape",shape="UsageCriteria")`).
pub const ACCOUNT_IDS_MEMBER: &str = "accountIds";

/// The wire name of `AccountDetail`'s email member
/// (`smithy_query(shape="AccountDetail")` renders `Email` as `email`).
pub const EMAIL_MEMBER: &str = "email";

/// The length of a GuardDuty detector id: the 32 hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] mints, which is the
/// form every id this service hands out has. The model's own constraint
/// (`DetectorId`, len 1..300 — `smithy_query(shape="DetectorId")`) is far wider
/// than the id format, which is why a syntactically impossible id reaches the
/// handler at all instead of being refused at the boundary.
pub const DETECTOR_ID_LEN: usize = 32;

/// Is `id` a syntactically well-formed id of the kind this service MINTS — the 32
/// hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] hands out?
///
/// Form only: a well-formed id need not exist. Case is not significant, so an id
/// a caller upper-cased is still well formed — and is then refused as not owned,
/// which is the ownership failure and not a parameter one. One reader for every
/// member of that form, so the form cannot drift between them.
pub fn is_minted_id(id: &str) -> bool {
    id.len() == DETECTOR_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Is `id` a syntactically well-formed `DetectorId`? See [`is_minted_id`].
pub fn is_detector_id(id: &str) -> bool {
    is_minted_id(id)
}

/// The message for a member whose VALUE cannot be a value of that member at all.
pub fn invalid_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

/// The message GuardDuty refuses a STRUCTURED member it will not accept with: the
/// member the request carried named, and nothing from inside the structure echoed.
///
/// A criteria structure is unacceptable as a WHOLE — the service does not report
/// which key inside it was at fault, nor repeat the key the caller sent — so the
/// refusal names the member the request carried and stops there.
pub fn invalid_parameter_value_message(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has an invalid value.")
}

/// The message GuardDuty refuses a criteria structure whose CONDITION VALUES it
/// will not take with: the member the request carried named, the fault located in a
/// condition, and nothing from inside the structure echoed.
///
/// It is a different sentence from [`invalid_parameter_value_message`] because it
/// is a different answer: the member's own shape is one the service takes, and what
/// it will not take is a value a condition inside it carries.
pub fn invalid_condition_values_message(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has invalid values in a condition.")
}

/// The message GuardDuty refuses an unusable input value with where it does NOT
/// name the member at fault: its generic parameter-validation wording.
///
/// Which of the two a member gets is the service's own choice, not ours: the
/// corpus records this wording for `GetMalwareScan`'s `scanId` and the
/// member-naming [`invalid_parameter_message`] for `detectorId`, so each member
/// keeps the message the service answers for it.
pub const INVALID_INPUT_VALUE_MESSAGE: &str =
    "The request is rejected because an invalid or out-of-range value is specified as an input parameter.";

/// The message for a member the request left out that the service cannot act
/// without.
pub fn missing_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} is required.")
}

/// The message for a well-formed id that this account does not own — an id
/// another account created, or one a delete removed. GuardDuty does not
/// distinguish "never existed" from "belongs to someone else": both are the same
/// refusal, which is also what keeps one account from probing another's ids.
///
/// It is the DETECTOR family's refusal and makes a claim about the ACCOUNT, so it
/// is not a generic not-found text: a family whose own refusal says only that
/// nothing of that name is there answers with that family's sentence instead —
/// [`NO_SUCH_RESOURCE_MESSAGE`], [`INVALID_FILTER_NAME_MESSAGE`],
/// [`MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE`], [`INVALID_FINDING_IDS_MESSAGE`]
/// or [`INVALID_INPUT_PARAMETERS_MESSAGE`].
pub fn not_owned_message(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// The sentence GuardDuty refuses an identifier that resolves to no resource with
/// on the families whose answer is that nothing of that name is there: ONE fixed
/// text for the whole class, naming neither the member that carried the identifier
/// nor the value itself.
///
/// The detector-rooted uploaded lists — an IPSet and a threat list, read, updated
/// or deleted — and a filter READ answer with this one, whatever the FORM of the
/// identifier they were handed: a value that was never minted, one minted under
/// another detector and one that could never have been minted at all are the same
/// absence, so the refusal depends on neither the store nor the spelling. It is not
/// [`not_owned_message`], which claims the id belongs to another ACCOUNT, and not
/// [`invalid_parameter_message`], which claims the value could not be an id at all.
pub const NO_SUCH_RESOURCE_MESSAGE: &str = "The request is rejected since no such resource found.";

/// How many Regions this store will hold detectors for.
///
/// "You can have only one detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`) bounds one PARTITION of
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::detectors`] to a single
/// detector, but the partition key is the Region read from the request's credential
/// scope ([`request_region`]), so the caller chooses it: without a ceiling on the
/// number of partitions, a caller inventing Region names could make the process
/// retain one detector per distinct Region string — and each detector carries up to
/// [`MAX_MEMBERS_PER_DETECTOR`] members, [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`]
/// filters and [`IP_SETS_PER_DETECTOR`] IPSets. Together with the one-per-Region
/// rule this is what makes the detector map finite whatever a caller sends.
///
/// AWS runs orders of magnitude fewer Regions than this, so a request signed for a
/// real Region never meets it; a caller inventing Region names does. Emulator-chosen,
/// like [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`], which bounds the plan store the same
/// way, and deliberately set ABOVE [`MAX_CLIENT_TOKENS`] so the two detector guards
/// compose rather than mask one another: the idempotency index evicts its oldest
/// mapping while the detector map is still partitioning, so neither bound hides the
/// other. Enforced by `CreateDetector` before it writes.
pub const MAX_DETECTOR_REGIONS: usize = 2 * MAX_CLIENT_TOKENS;

/// The message a create that would partition the detector store into one Region
/// more than [`MAX_DETECTOR_REGIONS`] is refused with. The caller's Region is not
/// quoted back, as with every other rejection here.
pub const DETECTOR_REGION_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of detectors has been reached.";

/// The message a second `CreateDetector` is refused with. "You can have only one
/// detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`); the Region is the one the
/// request was signed for, so the rejection does not name it.
pub const DETECTOR_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a detector already exists for the current account.";

/// Classify a minted id the store does not resolve, naming the member that carried
/// it: a value that is not an id of that form at all is a parameter failure, a
/// well-formed one this account/Region does not hold is an ownership failure.
///
/// One rule for every member that carries a minted id, so `detectorId` and
/// `threatIntelSetId` are refused the same way.
pub fn minted_id_rejection(member: &str, id: &str) -> String {
    if is_minted_id(id) {
        not_owned_message(member)
    } else {
        invalid_parameter_message(member)
    }
}

/// The length of an AWS account id: the 12 characters `AccountId` is constrained
/// to (`len 12..12` — `smithy_query(shape="AccountId")`). The model constrains the
/// LENGTH only, so a 12-character value that is not an account id at all reaches
/// the handler instead of being refused at the boundary.
pub const ACCOUNT_ID_LEN: usize = 12;

/// Is `id` a syntactically well-formed account id — exactly [`ACCOUNT_ID_LEN`]
/// decimal digits?
///
/// Form only: a well-formed id need not name an account this administrator has a
/// member record for, which is a different question and a different answer (see
/// [`NOT_AN_ASSOCIATED_MEMBER_MESSAGE`]).
pub fn is_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The message a value that cannot be an account id at all is refused with where the
/// request carried it in a STRUCTURE member of its own rather than in a list of ids:
/// the member name in single quotes, the format class stated, and nothing of the
/// caller's value echoed.
///
/// A malformed id is a FORMAT failure, which GuardDuty words as its own class rather
/// than as the generic unacceptable-value refusal
/// ([`invalid_parameter_message`]): a caller can tell a syntactically impossible
/// account id from one that was understood and rejected on its merits.
pub const ACCOUNT_ID_FORMAT_MESSAGE: &str =
    "The request failed because the 'accountId' was formatted incorrectly.";

/// The same format failure where the request carried the account ids as a LIST:
/// GuardDuty names the list `member` the caller sent and the `accountId` values
/// inside it, so the refusal says which parameter of a batch call it objected to.
///
/// `member` is the list's wire name as the request spells it
/// ([`ACCOUNT_IDS_MEMBER`] for the members operations, the include/exclude list for
/// an organization scope), so one sentence serves every batch that carries account
/// ids.
pub fn account_ids_format_message(member: &str) -> String {
    format!(
        "The request failed because parameter {member} has accountId values that are formatted incorrectly."
    )
}

/// Classify a LIST of account ids the caller named: `Some(message)` when an entry
/// cannot be an account id at all, `None` when every entry is well formed.
///
/// One reader for every operation that takes account ids, so the form cannot drift
/// between them, and one message — [`account_ids_format_message`] for the list
/// `member` the request carried — because a batch refusal names the list it was
/// handed. The whole call is refused rather than the entry reported:
/// `UnprocessedAccounts` reports accounts that were not PROCESSED, never a request
/// that was not understood.
///
/// The ids argument is any iterator of string-like entries, so an operation whose
/// scope is carried by TWO lists (an include list and an exclude list, which together
/// name one scope) chains them into one call; `member` then names the list the
/// request actually sent.
pub fn account_ids_rejection<I>(member: &str, account_ids: I) -> Option<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    account_ids
        .into_iter()
        .any(|account_id| !is_account_id(account_id.as_ref()))
        .then(|| account_ids_format_message(member))
}

/// Classify a SINGLE account id a structure member carried: `Some(message)` when it
/// cannot be an account id at all, `None` when it is well formed.
///
/// The same form question as [`account_ids_rejection`] under the sentence GuardDuty
/// answers when the id is a member of its own ([`ACCOUNT_ID_FORMAT_MESSAGE`]) rather
/// than an element of a list the request named.
pub fn account_id_rejection(account_id: &str) -> Option<&'static str> {
    (!is_account_id(account_id)).then_some(ACCOUNT_ID_FORMAT_MESSAGE)
}

/// Classify the top-level `ExpectedBucketOwner` a request carried: `Some(message)`
/// when the value supplied cannot be an account id at all, `None` when the request
/// omitted the member or supplied a well-formed id.
///
/// The member is "the Amazon Web Services account ID that owns the Amazon S3 bucket
/// specified in the location parameter"
/// (`smithy_query(query="doc",shape="CreateIPSetRequest",member="ExpectedBucketOwner")`),
/// so its value space is an account id's ([`is_account_id`]) and nothing wider.
/// Where the member targets `AccountId` the model constrains the LENGTH only, so a
/// 12-character value that is not an account id reaches the handler and the handler
/// is the enforcement point; where it targets a shape the model gives a
/// digits-only pattern the boundary refuses the same values first. One reader for
/// every operation that carries the member, so the form cannot drift between them,
/// and one message — [`invalid_parameter_message`] for
/// [`LIST_EXPECTED_BUCKET_OWNER_MEMBER`] — because the member at fault is the same
/// one whichever operation carried it.
pub fn expected_bucket_owner_rejection(owner: Option<&str>) -> Option<String> {
    owner
        .is_some_and(|owner| !is_account_id(owner))
        .then(|| invalid_parameter_message(LIST_EXPECTED_BUCKET_OWNER_MEMBER))
}

/// The account that owns the S3 bucket an ENTITY SET's location names: the one the
/// request supplied, or the account that owns the location when it supplied none.
///
/// `GetThreatEntitySet` and `GetTrustedEntitySet` report `ExpectedBucketOwner` as
/// "the Amazon Web Services account ID that owns the Amazon S3 bucket specified in
/// the location parameter"
/// (`smithy_query(query="doc",shape="GetThreatEntitySetResponse",member="ExpectedBucketOwner")`)
/// with no qualification on how the set was created — unlike `GetIPSet` and
/// `GetThreatIntelSet`, whose own documentation for the same member adds "this
/// field appears in the response only if it was provided during … creation or
/// update". So an entity set always KNOWS its bucket owner: a request that names
/// none is uploading from a bucket of its own account, which is the single account
/// this emulator serves ([`administrator_account_id`]), and that is what a read
/// reports and an update round-trips.
pub fn entity_set_bucket_owner(requested: Option<&str>) -> Option<String> {
    Some(
        requested
            .map(str::to_string)
            .unwrap_or_else(administrator_account_id),
    )
}

/// The DISTINCT entries of an id list a request carried, in the order the request
/// first named each of them.
///
/// An id list on this service names a SET of resources, so an id named twice names
/// the same one thing: the operations that answer such a list resolve each distinct
/// id once, and an operation that STORES the list normalises it here so a read of
/// the stored value reports each id once too. The request's order is kept, because
/// it is the order the stored list is reported in.
pub fn distinct_ids<I>(ids: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut seen = ::std::collections::BTreeSet::new();
    ids.into_iter()
        .map(|id| id.as_ref().to_string())
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// The administrator account the members of this emulator's detectors belong to:
/// the single account this emulator serves, which is the account every request it
/// answers is signed for.
///
/// It is NOT read from the request, so it is not caller input: a member's
/// `MasterId` / `AdministratorId` is the account that owns the detector the member
/// hangs off.
pub fn administrator_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// `UnprocessedAccount.Result` — "a reason why the account hasn't been processed"
/// (`smithy_query(shape="UnprocessedAccount")`) — for an account id that is not an
/// ASSOCIATED member of the administrator: one it holds no member record for, and
/// one whose record is not associated ([`MemberRecord::is_associated`]).
///
/// A per-account failure is reported here rather than refusing the whole request:
/// the operations that take an `AccountIds` list answer 200 and report the accounts
/// they could not process, which is what `UnprocessedAccounts` is for.
///
/// The administrator is named as "the current account", never by its account
/// number: the reason says WHICH account the membership is judged against, and the
/// account a request is signed for is the one it is answered for, so the sentence
/// is the same whatever account id this emulator serves.
pub const NOT_AN_ASSOCIATED_MEMBER_MESSAGE: &str = "The request is rejected because the given account ID is not an associated member of the current account.";

/// `UnprocessedAccount.Result` for an account `DeleteMembers` dropped no member
/// record for.
///
/// Deleting is what "remove[s] the details associated with a member account" that a
/// disassociation RETAINS (`smithy_query(query="doc",shape="DisassociateMembers")`),
/// so the record it does not drop is the one of a member GuardDuty MONITORS under
/// the administrator ([`MemberRecord::is_monitored`]): that member has to be
/// disassociated first, which is what this reason states. GuardDuty answers the SAME
/// reason for an account the administrator holds no member record for at all — there
/// was no member record to drop in either case — so it is the one reason for the
/// whole class, and it carries no account id.
pub const MEMBER_NOT_DELETED_MESSAGE: &str = "The request is rejected because the current account cannot delete the given member account ID since it is still associated to it.";

/// `UnprocessedAccount.Result` for an account `DisassociateMembers` was asked to
/// disassociate that is not ASSOCIATED with the administrator — one it holds no
/// member record for, one it has recorded and not invited yet, and one whose
/// relationship a disassociation already moved.
///
/// The condition is the association, not the record: the message states it ("the
/// latter is not yet associated to it"), and a disassociated member keeps its
/// record (see [`MemberRecord`]) while no longer being associated.
pub const NOT_ASSOCIATED_MESSAGE: &str = "The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.";

/// `UnprocessedAccount.Result` for the administrator's OWN account id.
///
/// `CreateMembers` "creates member accounts of the current Amazon Web Services
/// account by specifying a list of Amazon Web Services account IDs"
/// (`smithy_query(query="doc",shape="CreateMembers")`): the current account is the
/// administrator in that relationship, so it cannot also be one of its own members.
pub const SELF_AS_MEMBER_MESSAGE: &str =
    "The request is rejected because the current account cannot be used as a member account.";

/// `UnprocessedAccount.Result` for an account id `DeclineInvitations` had no
/// invitation to decline from.
///
/// Declining an invitation is a TRANSITION of the relationship with the account that
/// sent it: an invited account "can either accept or decline the invitation"
/// (`smithy_query(query="doc",shape="InviteMembers")`), so the transition is
/// available only while that invitation is pending. An account whose relationship
/// holds nothing to answer is therefore reported as a relationship-status transition
/// that cannot be made, in GuardDuty's FIXED sentence for that class: it asserts
/// nothing about which state the relationship was in and interpolates nothing of the
/// request or of state. The sibling discard reports its own reason
/// ([`INVITATION_NOT_DELETED_MESSAGE`]), so the two operations do not share one.
pub const INVALID_RELATIONSHIP_TRANSITION_MESSAGE: &str =
    "Relationship status transition is invalid.";

/// `UnprocessedAccount.Result` for an account id `DeleteInvitations` had no
/// invitation to discard from.
///
/// GuardDuty enumerates, in ONE fixed sentence, every condition under which an
/// invitation cannot be discarded — there is no such invitation, the current account
/// is still associated to that account, or it has not declined that account's
/// invitation yet — and deliberately does not say which of them held, so the reason
/// commits to none of them and carries nothing of the request or of state.
pub const INVITATION_NOT_DELETED_MESSAGE: &str = "The request is rejected either because no such invitation exists, or the current account is still associated to the given master account, or the current account has not yet declined the invitation from the given master account.";

/// How many member accounts one administrator's detector holds.
///
/// GuardDuty's own quota is 5,000 member accounts per administrator account per
/// Region, and the store is bounded by it rather than by nothing: a member record
/// is keyed by a REQUEST-chosen account id, so without a bound a caller repeating
/// `CreateMembers` with fresh ids could grow the store without limit. Dropping a
/// member is not an option the way evicting an old finding is — the records are
/// retained on purpose (see [`MemberRecord`]) — so the quota is enforced by
/// REFUSING the accounts past it, which is what the real quota does.
pub const MAX_MEMBERS_PER_DETECTOR: usize = 5_000;

/// `UnprocessedAccount.Result` for an account that would take the administrator
/// past [`MAX_MEMBERS_PER_DETECTOR`] member accounts.
pub fn member_quota_message() -> String {
    format!(
        "The request is rejected because the current account cannot have more than {MAX_MEMBERS_PER_DETECTOR} member accounts."
    )
}

/// The lower bound on an `AccountDetail.Email`: "the email address must be a
/// minimum of 6 and a maximum of 64 characters long"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). The model
/// carries `len 1..64`, so only the maximum is refused at the boundary.
pub const EMAIL_MIN_LEN: usize = 6;

/// The characters the local name of an email address cannot contain, as the member
/// documents them: "whitespace, \" ' ( ) < > [ ] : ' , \\ | % &"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). Whitespace
/// is checked separately, since it is a class rather than a character.
pub const EMAIL_LOCAL_FORBIDDEN: [char; 14] = [
    '"', '\'', '(', ')', '<', '>', '[', ']', ':', ',', '\\', '|', '%', '&',
];

/// Is `email` a valid `AccountDetail.Email`?
///
/// Every rule is one the member itself documents
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`), in the order
/// it lists them: 6..64 characters, 7-bit ASCII only, exactly one `@` separating
/// the local name from the domain name, no [`EMAIL_LOCAL_FORBIDDEN`] character or
/// whitespace and no leading dot in the local name, and a domain name of
/// `[a-zA-Z0-9]`, hyphen and dot only that neither begins nor ends with a dot or a
/// hyphen and contains at least one dot.
pub fn is_member_email(email: &str) -> bool {
    if email.len() < EMAIL_MIN_LEN || !email.is_ascii() {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let local_ok = !local.is_empty()
        && !local.starts_with('.')
        && !local
            .chars()
            .any(|c| c.is_whitespace() || EMAIL_LOCAL_FORBIDDEN.contains(&c));
    let domain_ok = !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain.contains('.');
    local_ok && domain_ok
}

/// `Member.RelationshipStatus` for an account `CreateMembers` has recorded and no
/// invitation has been sent to yet.
///
/// The member targets a plain `String` — "the status of the relationship between
/// the member and the administrator"
/// (`smithy_query(query="doc",shape="Member",member="RelationshipStatus")`) — so
/// the values are GuardDuty's documented relationship statuses, which are
/// mixed-case words rather than the SCREAMING_CASE of this service's modelled
/// enums.
pub const RELATIONSHIP_CREATED: &str = "Created";

/// `Member.RelationshipStatus` for a member the administrator disassociated with
/// `DisassociateMembers`. See [`RELATIONSHIP_CREATED`] for where the values come
/// from.
pub const RELATIONSHIP_REMOVED: &str = "Removed";

/// The relationship status of an invitation that has been delivered and not yet
/// answered: `Member.RelationshipStatus` once `InviteMembers` has sent the account
/// an invitation, and `Invitation.RelationshipStatus` on the invitation itself. See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_INVITED: &str = "Invited";

/// The relationship status of a member account that is being MONITORED:
/// `Master.RelationshipStatus` once the member account has accepted the
/// invitation, and `Member.RelationshipStatus` once `StartMonitoringMembers` has
/// turned monitoring back on for an account `StopMonitoringMembers` had stopped —
/// "use this operation to restart monitoring of accounts that you stopped
/// monitoring with the StopMonitoringMembers operation"
/// (`smithy_query(query="doc",shape="StartMonitoringMembers")`). See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_ENABLED: &str = "Enabled";

/// `Member.RelationshipStatus` for a member account the administrator has stopped
/// monitoring with `StopMonitoringMembers` — "stops GuardDuty monitoring for the
/// specified member accounts. Use the StartMonitoringMembers operation to restart
/// monitoring for those accounts"
/// (`smithy_query(query="doc",shape="StopMonitoringMembers")`). The member stays
/// ASSOCIATED ([`MemberRecord::is_associated`]): only `DisassociateMembers` ends
/// the association and only `DeleteMembers` drops the record. See
/// [`RELATIONSHIP_CREATED`] for where the values come from.
pub const RELATIONSHIP_DISABLED: &str = "Disabled";

/// `Member.RelationshipStatus` for a member whose invitation could not be
/// announced because the address `CreateMembers` recorded for it cannot be
/// verified ([`email_domain_receives_mail`]). See [`RELATIONSHIP_CREATED`] for
/// where the values come from.
///
/// It is the outcome of the SEND, not a status the clock later owes: GuardDuty
/// resolves the address's domain as it sends, so an address no mail can reach
/// fails verification there and then rather than staying pending for an answer
/// that can never arrive. The invitation itself is still delivered, so the
/// relationship is as associated as an answered-for one
/// ([`MemberRecord::is_associated`]) and the member account can still accept it
/// through the API — what failed is the EMAIL the invitation is announced by.
pub const RELATIONSHIP_EMAIL_VERIFICATION_FAILED: &str = "EmailVerificationFailed";

/// The DNS names no mail can be delivered to, as the names that are RESERVED
/// never to be assigned: the four special-use top-level domains `.invalid`,
/// `.test`, `.example` and `.localhost`, and the three second-level names
/// `example.com`, `example.net` and `example.org`, all of which are set aside for
/// documentation, examples and testing (RFC 2606 §2–3, RFC 6761 §6) and so have no
/// mail exchanger to hand a message to.
///
/// A domain IN one of these namespaces — the name itself or anything under it — is
/// the one class of address whose undeliverability is knowable without resolving
/// anything, which is why it is the whole catalogue: every other domain may have a
/// mail exchanger, so an invitation to it is announced and awaits an answer.
const UNDELIVERABLE_EMAIL_DOMAINS: [&str; 7] = [
    "invalid",
    "test",
    "example",
    "localhost",
    "example.com",
    "example.net",
    "example.org",
];

/// Can the domain of `email` receive the message an invitation is announced by?
///
/// Answers `false` only for an address in one of the reserved, never-assigned
/// namespaces ([`UNDELIVERABLE_EMAIL_DOMAINS`]), and for an address carrying no
/// domain at all; the comparison is case-insensitive and ignores a trailing root
/// dot, because neither distinguishes two DNS names.
pub fn email_domain_receives_mail(email: &str) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return false;
    }
    !UNDELIVERABLE_EMAIL_DOMAINS.iter().any(|reserved| {
        domain
            .strip_suffix(reserved)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

/// The relationship status an invitation sent to `email` leaves the member in:
/// [`RELATIONSHIP_INVITED`] once it has been announced and is awaiting the
/// account's answer, and [`RELATIONSHIP_EMAIL_VERIFICATION_FAILED`] when the
/// address cannot be verified at all.
///
/// One function for the send and for the re-send that must change nothing, so the
/// two cannot disagree on what the outcome of inviting one member is.
pub fn invitation_outcome(email: &str) -> &'static str {
    if email_domain_receives_mail(email) {
        RELATIONSHIP_INVITED
    } else {
        RELATIONSHIP_EMAIL_VERIFICATION_FAILED
    }
}

/// How many received invitations one detector's inbox retains.
///
/// A bound on [`Detector::received_invitations`], whose key is an ACCOUNT ID and
/// so request-chosen, exactly as [`MAX_MEMBERS_PER_DETECTOR`] bounds the
/// request-keyed member store. GuardDuty allows one administrator per member
/// account, so a member account has at most a handful of competing invitations
/// pending at once.
pub const MAX_INVITATIONS_PER_DETECTOR: usize = 50;

/// The wire name of the `MasterId` member — `jsonName` `masterId`
/// (`smithy_query(operation="AcceptInvitation")`). It is what a rejection names.
pub const MASTER_ID_MEMBER: &str = "masterId";

/// The wire name of the `InvitationId` member — `jsonName` `invitationId`
/// (`smithy_query(operation="AcceptInvitation")`).
pub const INVITATION_ID_MEMBER: &str = "invitationId";

/// The wire name of the `AdministratorId` member — `jsonName` `administratorId`
/// (`smithy_query(operation="AcceptAdministratorInvitation")`). The current name of
/// the member [`MASTER_ID_MEMBER`] carries on the deprecated twin, so a rejection
/// names whichever one the request used.
pub const ADMINISTRATOR_ID_MEMBER: &str = "administratorId";

/// The sentence an invitation acceptance is refused with when the administrator
/// account id and invitation id it carries identify no invitation pending for the
/// account that owns the named detector.
///
/// Both members are plain Strings with no modelled pattern and neither acceptance
/// declares a not-found shape — only `BadRequestException` (400) and
/// `InternalServerErrorException` (500)
/// (`smithy_query(operation="AcceptInvitation,AcceptAdministratorInvitation")`) — so
/// a pair that resolves to nothing is refused as an unusable input PARAMETER, under
/// the service's FIXED sentence for that class ([`INVALID_INPUT_VALUE_MESSAGE`]). It
/// names neither [`MASTER_ID_MEMBER`]/[`ADMINISTRATOR_ID_MEMBER`] nor
/// [`INVITATION_ID_MEMBER`], asserts no reason the lookup failed and interpolates
/// nothing of the request or of state: a value that could never be an account id and
/// a well-formed pair nobody invited are the one refusal, under either of the two
/// names the acceptance is bound under.
pub const INVITATION_NOT_PENDING_MESSAGE: &str = INVALID_INPUT_VALUE_MESSAGE;

/// The message the MEMBER-side disassociation is refused with when no OTHER account
/// administers the account that owns the named detector.
///
/// `DisassociateFromAdministratorAccount` "disassociates the current GuardDuty
/// member account from its administrator account"
/// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`): it
/// acts on the one relationship [`Detector::administrator`] holds, the one in which
/// the current account is the MEMBER. An account that no other account administers
/// administers itself — "a detector a create just minted administers itself", which is
/// why [`Detector::administrator`] starts empty — so such a request asks to leave a
/// relationship whose two sides are the same account, and GuardDuty refuses it with
/// this FIXED sentence naming that rule. It is the one refusal for the whole class:
/// an account that has accepted no invitation, one whose relationship a
/// disassociation already ended, and one that is itself the administrator of member
/// accounts all have no administrator other than themselves. The only client-error
/// channel the operation models is `BadRequestException`/400 under the
/// `InvalidInputException` class, and its output carries no per-account result the way
/// `DisassociateMembers`' `UnprocessedAccounts` does, so this is the refusal rather
/// than a hollow success. It names no member and interpolates nothing.
pub const SELF_DISASSOCIATION_MESSAGE: &str =
    "The request failed because admin account cannot disassociate from itself.";

/// The message a request is DENIED with when the account is not authorized on the
/// resource `member` names.
///
/// Like [`not_owned_message`], it names the member and not the caller's value: a
/// denial must not disclose whether the id names anything at all.
pub fn not_authorized_message(member: &str) -> String {
    format!(
        "The request is rejected because the current account is not authorized to perform this operation on the input {member}."
    )
}

/// Classify a `DetectorId` the store does not resolve: a value that is not a
/// detector id at all is a parameter failure, a well-formed one this
/// account/Region does not hold is an ownership failure.
pub fn detector_id_rejection(detector_id: &str) -> String {
    minted_id_rejection(DETECTOR_ID_MEMBER, detector_id)
}

/// The message a publishing-destination READ refuses a `DestinationId` it cannot
/// resolve with: `DescribePublishingDestination` reports a destination id that names
/// nothing as an unusable input PARAMETER ([`INVALID_INPUT_PARAMETERS_MESSAGE`])
/// rather than as an absent resource or a foreign one, and interpolates nothing — so
/// the one sentence answers an id a delete removed and one that could never have been
/// minted alike.
pub const DESTINATION_NOT_FOUND_MESSAGE: &str = INVALID_INPUT_PARAMETERS_MESSAGE;

/// The message the MUTATING publishing-destination paths refuse a `DestinationId`
/// they cannot resolve with: a write says the destination is not there, where the
/// read reports a bad parameter ([`DESTINATION_NOT_FOUND_MESSAGE`]) — the same
/// read-versus-write split the filter family makes
/// ([`NO_SUCH_RESOURCE_MESSAGE`] against [`INVALID_FILTER_NAME_MESSAGE`]).
///
/// Fixed text: the sentence names the parameter in AWS's own words for this refusal,
/// which is not the request's wire member name, and nothing of the caller's is
/// interpolated into it.
pub const DESTINATION_ABSENT_MESSAGE: &str =
    "The request failed because no publishingDestination exists with the provided publishingDestinationId.";

/// The wire name of the `FindingIds` member — `jsonName` `findingIds` on every
/// operation that addresses findings by id
/// (`smithy_query(operation="ArchiveFindings")`). It is what a rejection names.
pub const FINDING_IDS_MEMBER: &str = "findingIds";

/// The sentence a findings write is refused with when the batch names a finding
/// the detector does not hold — AWS's own wording for that class, its disagreement
/// in number included, and with none of the ids quoted back.
pub const INVALID_FINDING_IDS_MESSAGE: &str =
    "The request is rejected because an invalid finding identifiers is specified.";

/// Classify a finding id the detector does not hold: a finding `Id` is one of the
/// ids this service mints ([`is_minted_id`]), so a well-formed one the detector does
/// not hold is the finding-identifier refusal ([`INVALID_FINDING_IDS_MESSAGE`]),
/// while a value that is not of that form at all is a parameter failure — a value
/// the request could not have meant as a finding id, which is a different class
/// from a finding that is simply not this detector's.
pub fn finding_id_rejection(finding_id: &str) -> String {
    if is_minted_id(finding_id) {
        INVALID_FINDING_IDS_MESSAGE.to_string()
    } else {
        invalid_parameter_message(FINDING_IDS_MEMBER)
    }
}

/// The wire name of the `MalwareProtectionPlanId` member, as the request carries
/// it (`smithy_query(operation="GetMalwareProtectionPlan")` binds it as the URI
/// label `MalwareProtectionPlanId` and the create reports it as
/// `malwareProtectionPlanId`). It is what a rejection names.
pub const MALWARE_PROTECTION_PLAN_ID_MEMBER: &str = "malwareProtectionPlanId";

/// The wire name of `CreateMalwareProtectionPlan`'s `Role` member
/// (`smithy_query(operation="CreateMalwareProtectionPlan")` renders it `role`).
/// `UpdateMalwareProtectionPlan` carries the same member under the same wire name.
pub const ROLE_MEMBER: &str = "role";

/// The wire path of the `Role` a malware-scan start carries inside its
/// `ScanConfiguration` (`smithy_query(shape="StartMalwareScanConfiguration")`
/// renders the member `role`, under the request's `scanConfiguration`). Dotted the
/// way this service names a nested member in a rejection, as
/// [`DESTINATION_ARN_MEMBER`] is.
pub const SCAN_CONFIGURATION_ROLE_MEMBER: &str = "scanConfiguration.role";

/// The wire path of the `BackupVaultName` a malware-scan start carries inside its
/// `ScanConfiguration.RecoveryPoint` (`smithy_query(shape="RecoveryPoint")` renders
/// the member `backupVaultName`, under the request's
/// `scanConfiguration.recoveryPoint`). Dotted the way this service names a nested
/// member in a rejection, as [`SCAN_CONFIGURATION_ROLE_MEMBER`] is.
pub const SCAN_CONFIGURATION_RECOVERY_POINT_VAULT_MEMBER: &str =
    "scanConfiguration.recoveryPoint.backupVaultName";

/// The length bounds of `NonEmptyString` (`smithy_query(shape="NonEmptyString")`:
/// len 1..200) — the type a malware scan's configuration is REPORTED through
/// (`ScanConfigurationRecoveryPoint$BackupVaultName`,
/// `ScanConfiguration$Role`, `IncrementalScanDetails$BaselineResourceArn`).
pub const NON_EMPTY_STRING_MIN_LEN: usize = 1;
/// See [`NON_EMPTY_STRING_MIN_LEN`].
pub const NON_EMPTY_STRING_MAX_LEN: usize = 200;

/// Can `value` be carried by a `NonEmptyString` member — is its length within
/// [`NON_EMPTY_STRING_MIN_LEN`]..=[`NON_EMPTY_STRING_MAX_LEN`]?
///
/// A length bound counts Unicode code points, not bytes, so a multi-byte name is
/// measured the way the model measures it — the same counting
/// [`is_filter_name`] applies to the filter-name bounds. ONE reader for every
/// member of that type, so the bound cannot drift between the paths that must
/// honour it: where a request member the service persists is modelled LESS
/// constrained than the member a read reports the same fact through, the handler
/// is the only place the reporting member's bound can be honoured.
pub fn fits_non_empty_string(value: &str) -> bool {
    (NON_EMPTY_STRING_MIN_LEN..=NON_EMPTY_STRING_MAX_LEN).contains(&value.chars().count())
}

/// The wire name of the `ProtectedResource` member, for the same reason.
pub const PROTECTED_RESOURCE_MEMBER: &str = "protectedResource";

/// The wire name of `SendObjectMalwareScan`'s `S3Object` member
/// (`smithy_query(operation="SendObjectMalwareScan")` renders it `s3Object`), for a
/// rejection to name.
pub const S3_OBJECT_MEMBER: &str = "s3Object";

/// The wire name of that member's `Bucket`: "The name of the S3 bucket containing
/// the object to scan" (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`).
/// Dotted the way this service names a nested member in a rejection, as
/// [`DESTINATION_ARN_MEMBER`] is.
pub const S3_OBJECT_BUCKET_MEMBER: &str = "s3Object.bucket";

/// The wire name of that member's `Key`: "The key (name) of the S3 object to scan
/// for malware" (`smithy_query(shape="S3ObjectForSendObjectMalwareScan")`).
pub const S3_OBJECT_KEY_MEMBER: &str = "s3Object.key";

/// The message a send GuardDuty will not start a scan for is refused with.
///
/// ONE short fixed text for the whole class: the object the request named is not one
/// a plan of this account and Region scans — the bucket has no Malware Protection
/// plan ("the bucket must have a Malware Protection plan configured to use this
/// API",
/// `smithy_query(query="doc",shape="SendObjectMalwareScanRequest",member="S3Object")`),
/// or the key falls under none of that plan's object prefixes ("the S3 object will
/// be scanned only if it belongs to any of the specified object prefixes",
/// `smithy_query(query="doc",shape="CreateS3BucketResource",member="ObjectPrefixes")`).
/// GuardDuty does not report WHICH of them it was, and naming neither the bucket,
/// the key nor the prefixes a plan holds is also what keeps a caller from learning
/// through a refusal whether another account protects that bucket.
pub const OBJECT_MALWARE_SCAN_REFUSAL_MESSAGE: &str = "Unable to start a scan";

/// The `MalwareProtectionPlanStatus` a plan GuardDuty accepted reports: the plan
/// is in force and nothing is wrong with it. The enum's other two members
/// (`WARNING`, `ERROR` — `smithy_query(shape="MalwareProtectionPlanStatus")`) both
/// describe a plan whose role or bucket the service could not use, which is a
/// condition this emulator never produces, so every member of the enum is
/// terminal and a created plan needs no later transition.
pub const MALWARE_PROTECTION_PLAN_ACTIVE: &str = "ACTIVE";

/// The `MalwareProtectionPlanTaggingActionStatus` a plan gets when the create
/// names no `Actions`.
///
/// Tagging is what "will be added to the S3 object after scanning"
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`), and writing a tag
/// onto a scanned object needs a permission on the role beyond reading it, so a
/// plan that did not ask for it does not tag.
pub const DEFAULT_TAGGING_ACTION_STATUS: &str = DISABLED;

/// The message a create for an already-protected resource is refused with.
///
/// GuardDuty protects one S3 bucket with one plan, so the second create for the
/// same bucket conflicts with the first. AWS words this refusal in terms of the
/// RESOURCE the plan protects — the S3 bucket — in one fixed sentence; the
/// caller's bucket name is not quoted back.
pub const MALWARE_PROTECTION_PLAN_CONFLICT_MESSAGE: &str =
    "The request was rejected because the provided S3 bucket is already protected.";

/// How many Malware Protection plans one account may hold in one Region.
///
/// A plan is keyed by a REQUEST-chosen bucket name — the bucket is never required
/// to exist — so without a bound a caller naming fresh bucket names could grow the
/// store without limit. Neither the Smithy model nor this service's own
/// documentation carries a plans-per-account-per-Region figure, so unlike
/// [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`] (100, documented on `CreateFilter`) and
/// [`MAX_MEMBERS_PER_DETECTOR`] (5,000, GuardDuty's published member quota) this
/// number is an EMULATOR-CHOSEN bound, not a value AWS publishes: a 26th plan is
/// refused here, where the real service may accept it. The refusal states the rule
/// rather than the number, and it is what keeps the plan map, the plan idempotency
/// index (an entry is written only for a plan that was minted) and the per-create
/// duplicate scan bounded.
pub const MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION: usize = 25;

/// The message a create past
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] is refused with.
pub const MALWARE_PROTECTION_PLAN_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request is rejected because the maximum number of Malware Protection plans per account per Region has been reached.";

/// How many Regions this store will hold Malware Protection plans for.
///
/// [`MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION`] bounds each Region's
/// plans, but the Region is read from the REQUEST's credential scope
/// ([`request_region`]), so the caller chooses the partition KEY as well as what
/// goes in it: a per-Region quota bounds one partition and not the number of
/// partitions, so without this a caller varying the Region could grow the plan map
/// past every per-Region quota there is. AWS runs fewer Regions than this, so a
/// request signed for a real Region never meets it; a caller inventing Region
/// names does. Emulator-chosen, like the per-Region quota.
///
/// The detector-rooted store answers the same hazard the same way, which this does
/// not change: [`MAX_DETECTOR_REGIONS`] bounds how many Regions it partitions into,
/// a detector is one per Region, and its idempotency index is bounded by
/// [`MAX_CLIENT_TOKENS`] with eviction.
pub const MAX_MALWARE_PROTECTION_PLAN_REGIONS: usize = 50;

/// The most Malware Protection plans this store will ever hold: the per-Region
/// quota times the Region bound. It is what makes the plan map — and the plan
/// idempotency index, which gets an entry only for a plan that was minted — finite
/// whatever a caller sends.
pub const MAX_MALWARE_PROTECTION_PLANS_TOTAL: usize =
    MAX_MALWARE_PROTECTION_PLANS_PER_ACCOUNT_PER_REGION * MAX_MALWARE_PROTECTION_PLAN_REGIONS;

/// The message a create that would partition the plan store into one Region more
/// than [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`] is refused with. The caller's
/// Region is not quoted back, as with every other rejection here.
pub const MALWARE_PROTECTION_PLAN_TOTAL_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of Malware Protection plans has been reached.";

/// The sentence a plan this account and Region does not hold is refused with.
///
/// "Never existed" and "belongs to someone else" are ONE refusal, with one fixed
/// text and the plan id not quoted back, so one account cannot probe another's plan
/// ids — and the text says the plan was not found rather than that the caller does
/// not own it ([`not_owned_message`]), which would disclose the very distinction the
/// single wording hides.
pub const MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE: &str =
    "The request was rejected because the specified malware protection plan was not found.";

/// The message a read of a plan this account and Region does not hold answers
/// with: [`MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE`], owned by this one reader so
/// every plan operation refuses an unresolvable id the same way.
pub fn malware_protection_plan_not_found_message() -> String {
    MALWARE_PROTECTION_PLAN_NOT_FOUND_MESSAGE.to_string()
}

/// Is `value` an IAM role ARN?
///
/// `Role` is the "Amazon Resource Name (ARN) of the IAM role that has the
/// permissions to scan and add tags to the associated protected resource"
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`), and the model
/// constrains it no further than `String`, so the form is checked here:
/// ONE reader for every request member that carries an IAM role ARN — the plan
/// create and update, and a malware scan's `ScanConfiguration.Role`, "the IAM role
/// that is used for scanning the resource"
/// (`smithy_query(shape="StartMalwareScanConfiguration")`) — so the form cannot
/// drift between the paths that take one:
/// `arn:<partition>:iam::<account>:role/<name>`. IAM is global, so a role ARN's
/// Region segment is empty. Form only — the role need not exist, and its account
/// need not be the caller's.
pub fn is_iam_role_arn(value: &str) -> bool {
    let mut parts = value.splitn(6, ':');
    parts.next() == Some("arn")
        && parts.next().is_some_and(|partition| !partition.is_empty())
        && parts.next() == Some("iam")
        && parts.next() == Some("")
        && parts
            .next()
            .is_some_and(|account| !account.is_empty() && account.bytes().all(|b| b.is_ascii_digit()))
        && parts
            .next()
            .and_then(|resource| resource.strip_prefix("role/"))
            .is_some_and(|name| !name.is_empty())
}

/// The ARN of the Malware Protection plan `id` in `region`.
///
/// Derived, never stored: the plan's own id and the Region that serves it are the
/// only variable parts, so a read cannot report an ARN that disagrees with the id
/// it was asked about.
pub fn malware_protection_plan_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        GUARDDUTY_ARN_SERVICE,
        region,
        &format!("malware-protection-plan/{id}"),
    )
}

/// This service's segment in an ARN it owns — the `arnNamespace`/`endpointPrefix`
/// from `smithy_query(query="service_info")`.
const GUARDDUTY_ARN_SERVICE: &str = "guardduty";

/// The value `Service.ServiceName` carries on every finding: "The name of the Amazon
/// Web Services service (GuardDuty) that generated a finding"
/// (`smithy_query(shape="Service")`).
///
/// Held beside the finding record because both the reported finding and the criterion
/// resolver read it, so a filter on `service.serviceName` cannot disagree with the
/// value a findings read reports.
pub const GUARDDUTY_SERVICE_NAME: &str = "guardduty";

/// The wire name of the filter-name member on the operations that carry it as a
/// URI label — `GetFilter`, `UpdateFilter` and `DeleteFilter` bind
/// `FilterName` to `/detector/{DetectorId}/filter/{FilterName}`
/// (`smithy_query(operation="GetFilter")`). The filter family's own refusals name no
/// member ([`NO_SUCH_RESOURCE_MESSAGE`], [`INVALID_FILTER_NAME_MESSAGE`]), so this is
/// the wire name itself and not a rejection's subject.
pub const FILTER_NAME_MEMBER: &str = "filterName";

/// The wire name `CreateFilter` carries the filter name under: its `Name` member
/// has `jsonName` `name` (`smithy_query(operation="CreateFilter")`). A rejection that
/// names a member names it AS THE REQUEST CARRIES IT, which is why a create and a read
/// would name a filter name differently.
pub const CREATE_FILTER_NAME_MEMBER: &str = "name";

/// The `FilterName` length bounds (`smithy_query(shape="FilterName")`: len
/// 3..64). `GetFilter`/`UpdateFilter`/`DeleteFilter` take the name as a plain
/// `String` URI label with no constraint of its own, so a name outside these
/// bounds reaches the handler there and is classified by [`is_filter_name`].
pub const FILTER_NAME_MIN_LEN: usize = 3;
/// See [`FILTER_NAME_MIN_LEN`].
pub const FILTER_NAME_MAX_LEN: usize = 64;

/// Is `name` a syntactically well-formed `FilterName`?
///
/// "Valid characters include period (.), underscore (_), dash (-), and
/// alphanumeric characters. A whitespace is considered to be an invalid
/// character" (`smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`),
/// within the modelled length bounds. Form only: a well-formed name need not
/// exist.
pub fn is_filter_name(name: &str) -> bool {
    (FILTER_NAME_MIN_LEN..=FILTER_NAME_MAX_LEN).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The sentence the MUTATING filter paths refuse a `FilterName` they cannot act on
/// with: `UpdateFilter` and `DeleteFilter` answer that the name they were given is
/// not a usable one, where the READ answers that no such resource was found
/// ([`NO_SUCH_RESOURCE_MESSAGE`]). Fixed text — the name is not quoted back — and it
/// is the answer for a name the detector does not hold and for a value that could
/// not be a filter name at all alike.
pub const INVALID_FILTER_NAME_MESSAGE: &str =
    "The request is rejected because the given filter name is invalid.";

/// The message a second `CreateFilter` under a name the detector already holds
/// is refused with. Creating is not updating — `UpdateFilter` is the operation
/// that changes an existing filter — and the rejection does not quote the name
/// back at the caller.
pub const FILTER_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a filter with the same name already exists.";

/// "The maximum number of saved filters per Amazon Web Services account per
/// Region is 100" (`smithy_query(query="doc",shape="CreateFilter")`). A detector
/// is itself one per account per Region, so the per-detector count IS the
/// per-account-per-Region count.
pub const MAX_FILTERS_PER_ACCOUNT_PER_REGION: usize = 100;

/// The message a `CreateFilter` past [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`] is
/// refused with.
pub const FILTER_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request is rejected because the maximum number of saved filters per account per Region has been reached.";

/// The `FilterAction` a filter gets when the request names none: "Default: NOOP"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`).
pub const FILTER_ACTION_NOOP: &str = "NOOP";

/// The `FilterAction` that SUPPRESSES: "the action that is to be applied to the
/// findings that match the filter"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`) is one
/// of two values (`smithy_query(shape="FilterAction")`), and this is the one that
/// changes a finding — it archives every finding the filter's criteria select
/// ([`Detector::apply_filter_action`]). The other names no change at all.
pub const FILTER_ACTION_ARCHIVE: &str = "ARCHIVE";

/// The `FilterVersion` a freshly created filter carries. `FilterVersion` is
/// `range min 1` (`smithy_query(shape="FilterVersion")`), so a filter nothing has
/// updated is at version 1.
pub const FILTER_INITIAL_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Modelled-constraint refusals
// ---------------------------------------------------------------------------

/// The sentence GuardDuty refuses a value outside a constraint its model declares
/// with: ONE fixed text for the whole class, naming no member and interpolating
/// neither the bound that was broken nor the value that broke it.
///
/// The bound is already published in the model the caller built its request from, so
/// the refusal restates nothing: it says only that some input parameter held a value
/// the service will not take. A client that branches on this text, or snapshots it,
/// sees the same sentence whichever member and whichever bound was at fault.
pub const INVALID_INPUT_PARAMETERS_MESSAGE: &str =
    "The request is rejected because the one or more input parameters have invalid values.";

/// The sentence GuardDuty refuses a value its request payload could not be read as
/// with — the answer to a payload that did not DESERIALIZE into the modelled input,
/// which is what a value outside an enumerated type's value set is.
///
/// It is a different class from [`INVALID_INPUT_PARAMETERS_MESSAGE`]: the value was
/// not understood at all rather than understood and rejected, so the allowed values
/// are not enumerated back at the caller and a client reading this text knows to
/// re-serialize rather than to re-think the value.
pub const UNPROCESSABLE_PAYLOAD_MESSAGE: &str =
    "The request is rejected because the JSON could not be processed.";

/// How GuardDuty states a refusal the request boundary made: a member the model
/// requires was absent, or a value fell outside a bound the model declares.
///
/// GuardDuty's model declares exactly two errors for every operation —
/// `BadRequestException` (`httpError` 400) and `InternalServerErrorException`
/// (500) (`smithy_query(query="service_info")` lists them as the service's common
/// errors) — and NO `ValidationException` shape. So a modelled-constraint
/// violation is the same refusal as any other unacceptable input, under one of the
/// two FIXED texts above, chosen by whether the value was unreadable or merely
/// unacceptable; neither quotes the caller's value, the member at fault or the
/// bound back.
///
/// `service.rs` routes the framework's pre-handler refusal through here, so a
/// constraint the model declares on ANY member of ANY operation in this service
/// is refused under the modelled error rather than under a shape no GuardDuty
/// client can catch.
pub fn constraint_rejection(violations: &[::aws_smithy_schema_server::Violation]) -> String {
    use ::aws_smithy_schema_server::ViolationKind;

    // A value outside an enumerated type's value set is not a value of the member's
    // modelled type at all, so reading the payload into that input is what failed.
    if violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)))
    {
        return UNPROCESSABLE_PAYLOAD_MESSAGE.to_string();
    }
    // An ABSENT member is not a value at all, so neither text above describes it:
    // the service states which member it cannot act without, as it does for a
    // member a handler finds missing.
    if let Some(absent) = violations
        .iter()
        .find(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember))
    {
        // The path is already the member's wire spelling (lowerCamel, with
        // `.member` steps for collections), which is how the caller named it.
        return missing_parameter_message(absent.path.as_str());
    }
    // Every remaining class — a length, a range, a pattern, uniqueness, and any
    // class `ViolationKind` (which is `#[non_exhaustive]`) gains later — is a value
    // the service will not take, and they all share the one sentence.
    INVALID_INPUT_PARAMETERS_MESSAGE.to_string()
}

/// The modelled server fault a handler answers when the engine it was built over
/// holds no state for this service.
///
/// `Type` is "The error type" (`smithy_query(shape="InternalServerErrorException")`)
/// — the error class, the same value the body's `__type` discriminator carries.
pub fn state_unavailable() -> crate::ops::types::shared_types::InternalServerErrorException {
    crate::ops::types::shared_types::InternalServerErrorException {
        message: Some("the GuardDuty state store is unavailable".to_string()),
        r#type: Some("InternalServerErrorException".to_string()),
    }
}

/// A configured detector feature and when its status last changed.
///
/// Compared by value, so a write over a stored configuration can answer whether it
/// moved anything: re-applying the status a switch already carries leaves an EQUAL
/// record behind ([`set_feature_in`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
    pub additional_configuration: Vec<AdditionalConfigRecord>,
    /// When this feature's free trial began for the account that holds this
    /// record, in epoch milliseconds, or `None` while the feature has never been
    /// enabled for it and its trial has therefore not started.
    ///
    /// Recorded ONCE, at the instant the switch first becomes [`ENABLED`], and kept
    /// across every later switch: the trial is a one-off per account per feature, so
    /// turning a feature off and on again does not buy a second one. It is a
    /// different fact from [`Self::updated_at_millis`], which moves every time the
    /// switch changes, and it is the single stored fact
    /// [`free_trial_days_remaining`] counts the remaining days from.
    pub free_trial_started_at_millis: Option<i64>,
}

/// A feature's nested additional configuration (e.g. a Runtime Monitoring agent
/// management switch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditionalConfigRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
}

/// One `ScanConditionPair`: a tag key and the value that condition matches.
///
/// `Value` is optional while `Key` is required
/// (`smithy_query(shape="ScanConditionPair")`), so a pair that names only a key is
/// stored as it arrived rather than being filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTagPair {
    pub key: String,
    pub value: Option<String>,
}

/// One `ScanCriterion`: each `ScanCriterionKey` mapped to the `MapEquals` pairs that
/// criterion matches. A `BTreeMap` so a read reports the criteria in a total order
/// whatever order the request listed them in.
pub type ScanCriterionRecord = BTreeMap<String, Vec<ScanTagPair>>;

/// A detector's Malware Protection scan settings: what
/// `UpdateMalwareScanSettings` writes and `GetMalwareScanSettings` reports.
///
/// `Include` is the "condition that when matched will allow a malware scan for a
/// certain resource" and `Exclude` the one that "will prevent" it
/// (`smithy_query(shape="ScanResourceCriteria")`), so the two are kept apart rather
/// than folded into one list.
#[derive(Debug, Clone)]
pub struct MalwareScanSettings {
    /// One of the `EbsSnapshotPreservation` members the generated
    /// [`crate::ops::types::shared_types::EBS_SNAPSHOT_PRESERVATION_VALUES`] lists;
    /// [`NO_RETENTION`] until an update asks for the other one.
    pub ebs_snapshot_preservation: String,
    /// `ScanResourceCriteria.Include`, empty when the detector has no inclusion
    /// criteria.
    pub include: ScanCriterionRecord,
    /// `ScanResourceCriteria.Exclude`, empty when the detector has no exclusion
    /// criteria.
    pub exclude: ScanCriterionRecord,
}

impl Default for MalwareScanSettings {
    /// The settings a detector has before any `UpdateMalwareScanSettings`: no scan
    /// criteria, so every eligible resource is in scope, and snapshots retained for
    /// nothing.
    fn default() -> Self {
        Self {
            ebs_snapshot_preservation: NO_RETENTION.to_string(),
            include: BTreeMap::new(),
            exclude: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ListMembers
// ---------------------------------------------------------------------------

/// The wire name of `ListMembers`'s `OnlyAssociated` query parameter
/// (`smithy_query(operation="ListMembers")`), for the same reason as
/// [`DETECTOR_ID_MEMBER`]: it is what a rejection names.
pub const ONLY_ASSOCIATED_MEMBER: &str = "onlyAssociated";

/// How GuardDuty refuses an `AccountDetail.Email` that is not formatted the way
/// [`is_member_email`]'s documented rules require: the service's own wording for
/// that rule, which names the member it refuses rather than echoing the value.
pub const MEMBER_EMAIL_FORMAT_MESSAGE: &str =
    "The request failed because provided `emailId` was formatted incorrectly.";

/// The AWS Region a request was signed for, which is the Region the resources it
/// creates live in. Falls back to the emulator's default Region for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGuarddutyGuarddutyapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGuarddutyGuarddutyapiserviceState>)
}
