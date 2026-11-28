// SPDX-License-Identifier: CC0-1.0

//! The buyer-organization approvals an agreement request goes through: the
//! approval policy this emulator publishes, the approvals `StartAgreementApprovals`
//! mints from it, and the outcomes `AcceptAgreementApproval` and
//! `RejectAgreementApproval` record on them.
//!
//! One record per approval ([`AgreementApproval`]), holding only what is about the
//! APPROVAL: who has to decide, what each of them decided and when the decision is
//! needed by. Everything else a report of an approval states — the proposer, the
//! proposal and the charges — is read from the agreement request the approval is
//! for, so the two cannot disagree.
//!
//! The approval POLICY — which plans apply to a request, the levels of each plan
//! and the approvers at each level — is DATA this emulator publishes, on exactly
//! the same footing as the offer document and the rate card in the module root:
//! an AWS Marketplace buyer configures it outside this API (this model has no
//! operation that writes one), so there is nothing to read it from and nothing to
//! compute it from either. [`APPROVAL_POLICY`] is therefore the published policy of
//! the organization this emulator serves, and every approval it mints is minted
//! from it.

use super::*;

use crate::ops::types::{
    AccessDeniedException, AgreementApprovalPlan, AgreementApprovalProposalSummary,
    AgreementApprovalProposer,
    AgreementApprovalPurchaseOrder, AgreementApprovalRequester, AgreementApprovalResource,
    AgreementApprovalSummary, Approver, ApproverOutcome, AwsOrganizationAdminApprover,
    AwsOrganizationApproverOutcome, BuyerAgreementApproval, BuyerAgreementApprovalCriteria,
    BuyerAgreementApprovalGroup, BuyerAgreementApprovalLevel, BuyerAgreementApprovalPlan,
    BuyerAgreementApprovalPlanGroup, BuyerAgreementApprovalPlanInput,
    BuyerAgreementApprovalPlanLevel, CallerIdentity, EmailApprover, EmailApproverOutcome,
    ProcurementToolApprover, ProcurementToolApproverOutcome,
};

/// `AgreementApprovalType`'s only value: "an approval that the buyer's
/// organization requires before the agreement request can be accepted".
pub const APPROVAL_TYPE_BUYER: &str = "BUYER_AGREEMENT_APPROVAL";

/// The `AgreementApprovalStatus` an approval is started in, and the only one a
/// decision can be recorded on.
pub const APPROVAL_STATUS_PENDING: &str = "PENDING_APPROVAL";
/// The `AgreementApprovalStatus` an approval reaches "once every level is
/// approved".
pub const APPROVAL_STATUS_APPROVED: &str = "APPROVED";
/// The `AgreementApprovalStatus` one rejection moves an approval to: "once
/// rejected, the agreement approval and its agreement request transition to
/// `REJECTED` status".
pub const APPROVAL_STATUS_REJECTED: &str = "REJECTED";
/// The `AgreementApprovalStatus` an approval nobody decided by its deadline
/// reaches — "the date and time when the agreement approval expires if it isn't
/// approved or rejected" ([`AgreementApproval::settle`]).
pub const APPROVAL_STATUS_EXPIRED: &str = "EXPIRED";
/// The `AgreementApprovalStatus` cancelling the agreement request moves its open
/// approvals to: "once cancelled, the agreement request transitions to
/// `CANCELLED` status and its open agreement approvals are cancelled".
pub const APPROVAL_STATUS_CANCELLED: &str = "CANCELLED";

/// The `ApproverOutcomeStatus` of an approver whose level has not been reached:
/// "approvers at a level can act only after the previous level is approved".
pub const OUTCOME_QUEUED: &str = "QUEUED";
/// The `ApproverOutcomeStatus` of an approver whose level is the one awaiting a
/// decision.
pub const OUTCOME_PENDING: &str = "PENDING_APPROVAL";
/// The `ApproverOutcomeStatus` of an approver who approved.
pub const OUTCOME_APPROVED: &str = "APPROVED";
/// The `ApproverOutcomeStatus` of an approver who rejected.
pub const OUTCOME_REJECTED: &str = "REJECTED";
/// The `ApproverOutcomeStatus` of an approver whose decision is no longer needed:
/// the group reached the approvals its criteria require, or the approval was
/// decided without them.
pub const OUTCOME_SKIPPED: &str = "SKIPPED";
/// The `ApproverOutcomeStatus` of an approver who had not decided when the
/// approval expired.
pub const OUTCOME_EXPIRED: &str = "EXPIRED";

/// `AgreementApprovalPartyType`'s two values: the buyer that CREATED the
/// agreement request, and the administrators of its AWS organization.
pub const APPROVAL_PARTY_REQUESTER: &str = "REQUESTER";
/// See [`APPROVAL_PARTY_REQUESTER`].
pub const APPROVAL_PARTY_ORGANIZATION_ADMIN: &str = "AWS_ORGANIZATION_ADMIN";

/// `ResourceType` values the approval operations report on a
/// `ResourceNotFoundException` or a `ConflictException`.
pub const RESOURCE_TYPE_AGREEMENT_APPROVAL: &str = "AgreementApproval";
/// See [`RESOURCE_TYPE_AGREEMENT_APPROVAL`].
pub const RESOURCE_TYPE_APPROVAL_PLAN: &str = "AgreementRequestApprovalPlan";

/// The AWS organization the buyer this emulator serves belongs to, as
/// `OrganizationId` spells one (`^o-[a-z0-9]{10,32}$`).
///
/// SOURCE: the emulator's OWN value, like the rest of [`APPROVAL_POLICY`]. An
/// organization is a resource of AWS Organizations rather than of this service,
/// and this model carries no member a caller could state one with, so the
/// organization whose administrators decide for this buyer is published here.
pub const BUYER_ORGANIZATION_ID: &str = "o-emulatorbuyer1";

/// How long an approval this service starts stays open: the window between the
/// instant approvers could start acting on it (`startedAt`) and the instant it
/// "must be completed" by (`approvalNeededBy`), after which it "expires if it
/// isn't approved or rejected" (`expiresAt`).
///
/// AWS publishes no number for it, so this is the emulator's OWN window — the two
/// members report one deadline because there is one, and an approval still
/// undecided past it is reported `EXPIRED` ([`AgreementApproval::settle`]) rather
/// than left open for ever.
pub const APPROVAL_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many agreement approvals the store holds before `StartAgreementApprovals`
/// refuses to start another.
///
/// The same kind of bound as [`AGREEMENT_REQUEST_QUOTA`] and for the same reason:
/// AWS publishes no number, so it is the emulator's OWN bound on the store —
/// what stops state growing without end — rather than a claim about an account's
/// quota, and it is far above any workflow an emulator serves.
/// `StartAgreementApprovals` models no `ServiceQuotaExceededException`, so a start
/// past the bound is refused with its modelled `ThrottlingException`
/// ([`store_full`]) and nothing is stored.
pub const AGREEMENT_APPROVAL_QUOTA: usize = 1_000;

/// One approver of a published approval plan.
///
/// The three kinds are the ones `Approver` models: the administrators of the
/// buyer's AWS organization, an approver identified by an email address, and the
/// buyer's procurement system.
#[derive(Debug, Clone, Copy)]
pub enum PublishedApprover {
    /// "An approver that represents the administrators of the buyer's AWS
    /// organization. The management account or a delegated administrator of the
    /// organization can decide for this approver."
    AwsOrganizationAdmin,
    /// "An approver who is identified by an email address", with the name the
    /// policy publishes for them.
    Email {
        address: &'static str,
        name: &'static str,
    },
    /// "A procurement system that approves through the buyer's procurement
    /// integration."
    ProcurementTool { name: &'static str },
}

/// One level of a published approval plan: the approvers at that level and how
/// many of them have to approve.
#[derive(Debug, Clone, Copy)]
pub struct PublishedApprovalLevel {
    /// `BuyerAgreementApprovalPlanLevel$name`.
    pub name: &'static str,
    /// `BuyerAgreementApprovalCriteria$requiredApprovalCount`: "the number of
    /// approvers in the group who must approve before the level is approved".
    pub required_approval_count: i32,
    /// The approvers of the level's one group — `BuyerAgreementApprovalGroups` is
    /// `@length(min: 1, max: 1)`, so a level has exactly one.
    pub approvers: &'static [PublishedApprover],
}

/// One approval plan of the published policy.
#[derive(Debug, Clone, Copy)]
pub struct PublishedApprovalPlan {
    /// `^aplan-[A-Za-z0-9]+$`: "the identifier of the approval plan. Pass it in
    /// `StartAgreementApprovals` to start an agreement approval for this plan."
    pub reference: &'static str,
    /// `BuyerAgreementApprovalPlan$name`, which is also the name of every
    /// approval started for the plan.
    pub name: &'static str,
    /// The least agreement value a request has to commit for this plan to apply
    /// to it, in the units [`amount_units`] counts. A plan whose threshold is
    /// zero applies to every request.
    pub applies_from_units: u128,
    /// Whether the policy MAKES approval mandatory for a request this plan
    /// applies to — what `ApprovalRequirement` reports ("indicates whether the
    /// agreement request requires approval before it can be accepted").
    pub mandatory: bool,
    /// "The approval levels of the plan, in the order they are evaluated."
    pub levels: &'static [PublishedApprovalLevel],
}

/// The approval policy the buyer's organization publishes: every plan a request
/// of this account can be approved under.
///
/// SOURCE: the emulator's OWN policy — see the module docs for why there is
/// nothing to read one from. The shape of it is what the model describes: a plan
/// of one to six levels, each level one group of one to ten approvers with the
/// number of them that has to approve.
///
/// The base plan applies to every request, so `GetAgreementRequestApprovalPlans`
/// — whose `items` is `@required` and `@length(min: 1)` — always has a plan to
/// report, and a buyer can route any purchase through its administrators. The
/// second plan applies to a request that commits [`MANDATORY_APPROVAL_FROM_UNITS`]
/// or more and is MANDATORY: such a request cannot be accepted until an approval
/// of it is approved.
pub const APPROVAL_POLICY: &[PublishedApprovalPlan] = &[
    PublishedApprovalPlan {
        reference: "aplan-organizationadmin",
        name: "Organization administrator approval",
        applies_from_units: 0,
        mandatory: false,
        levels: &[PublishedApprovalLevel {
            name: "Organization administrator",
            required_approval_count: 1,
            approvers: &[PublishedApprover::AwsOrganizationAdmin],
        }],
    },
    PublishedApprovalPlan {
        reference: "aplan-highvaluepurchase",
        name: "High value purchase approval",
        applies_from_units: MANDATORY_APPROVAL_FROM_UNITS,
        mandatory: true,
        levels: &[
            PublishedApprovalLevel {
                name: "Organization administrator",
                required_approval_count: 1,
                approvers: &[PublishedApprover::AwsOrganizationAdmin],
            },
            PublishedApprovalLevel {
                name: "Finance review",
                required_approval_count: 2,
                approvers: &[
                    PublishedApprover::Email {
                        address: "finance-approver@example.com",
                        name: "Finance approver",
                    },
                    PublishedApprover::Email {
                        address: "controller@example.com",
                        name: "Controller",
                    },
                    PublishedApprover::ProcurementTool {
                        name: "Emulated procurement system",
                    },
                ],
            },
        ],
    },
];

/// The agreement value from which the published policy makes approval mandatory,
/// in the units [`amount_units`] counts.
///
/// SOURCE: the emulator's OWN threshold, part of [`APPROVAL_POLICY`] — a buyer
/// sets its own, and this service has no member to read one from. It is stated in
/// whole currency units of [`CURRENCY_CODE`] so the policy reads as a buyer writes
/// one.
pub const MANDATORY_APPROVAL_FROM_UNITS: u128 = 50_000 * AMOUNT_UNITS_PER_CURRENCY_UNIT;

/// The value a quote commits, as the policy measures a request: the agreement
/// value of its charge summary, and zero where that is not a value this service
/// can count.
///
/// `ChargeSummary$newAgreementValue` is absent for a quote whose terms the
/// published rates do not price, and a value nothing knows cannot reach a
/// threshold — such a request is measured as committing nothing rather than as
/// committing everything.
pub fn committed_units(charge_summary: &ChargeSummary) -> u128 {
    charge_summary
        .new_agreement_value
        .as_deref()
        .and_then(amount_units)
        .unwrap_or(0)
}

/// The plans of [`APPROVAL_POLICY`] that apply to a request committing
/// `value_units`: every plan whose own threshold that value reaches, in the order
/// the policy publishes them.
pub fn plans_for(value_units: u128) -> Vec<&'static PublishedApprovalPlan> {
    APPROVAL_POLICY
        .iter()
        .filter(|plan| value_units >= plan.applies_from_units)
        .collect()
}

/// The published plan `reference` names among the plans that apply to a request
/// committing `value_units`, or `None` when the policy publishes no such plan for
/// it.
pub fn applying_plan(
    value_units: u128,
    reference: &str,
) -> Option<&'static PublishedApprovalPlan> {
    plans_for(value_units)
        .into_iter()
        .find(|plan| plan.reference == reference)
}

/// `ApprovalRequirement` for a request committing `value_units`: `REQUIRED` when
/// a plan the policy makes mandatory applies to it, and `NOT_REQUIRED` otherwise.
pub fn approval_requirement(value_units: u128) -> &'static str {
    if plans_for(value_units).iter().any(|plan| plan.mandatory) {
        APPROVAL_REQUIRED
    } else {
        APPROVAL_NOT_REQUIRED
    }
}

/// One published plan as `GetAgreementRequestApprovalPlans` reports it.
pub fn plan_report(plan: &PublishedApprovalPlan) -> AgreementApprovalPlan {
    AgreementApprovalPlan::buyerAgreementApprovalPlan(BuyerAgreementApprovalPlan {
        plan_reference: plan.reference.to_string(),
        name: Some(plan.name.to_string()),
        levels: plan
            .levels
            .iter()
            .map(|level| BuyerAgreementApprovalPlanLevel {
                name: Some(level.name.to_string()),
                groups: vec![BuyerAgreementApprovalPlanGroup {
                    approval_criteria: BuyerAgreementApprovalCriteria {
                        required_approval_count: level.required_approval_count,
                    },
                    approvers: level.approvers.iter().map(published_approver).collect(),
                }],
            })
            .collect(),
    })
}

/// One published approver as a plan reports it.
fn published_approver(approver: &PublishedApprover) -> Approver {
    match approver {
        PublishedApprover::AwsOrganizationAdmin => {
            Approver::awsOrganizationAdminApprover(AwsOrganizationAdminApprover {
                organization_id: BUYER_ORGANIZATION_ID.to_string(),
            })
        }
        PublishedApprover::Email { address, name } => Approver::emailApprover(EmailApprover {
            email_address: (*address).to_string(),
            name: Some((*name).to_string()),
        }),
        PublishedApprover::ProcurementTool { name } => {
            Approver::procurementToolApprover(ProcurementToolApprover {
                name: (*name).to_string(),
            })
        }
    }
}

/// WHO one approver of a started approval is, as the published policy named them.
#[derive(Debug, Clone)]
pub enum ApproverIdentity {
    /// The administrators of the buyer's AWS organization — the approver the
    /// caller of `AcceptAgreementApproval` and `RejectAgreementApproval` decides
    /// AS, those operations being for "the management account or a delegated
    /// administrator of the buyer's AWS organization".
    AwsOrganizationAdmin { organization_id: String },
    /// An approver identified by an email address. Nothing in this API lets such
    /// an approver call it — they decide out of band — so their outcome moves only
    /// when an organization administrator decides "on behalf of another approver"
    /// ([`AgreementApproval::decide`]). The address and the name are `@sensitive`
    /// members of this model, so the record holds them where a formatter cannot
    /// print them ([`Sensitive`]).
    Email {
        address: Sensitive<String>,
        name: Option<Sensitive<String>>,
    },
    /// The buyer's procurement system, whose outcome moves the same way an
    /// email approver's does. `ApproverName` is `@sensitive` too.
    ProcurementTool { name: Sensitive<String> },
}

impl ApproverIdentity {
    /// The approver the published policy names, as a started approval holds them.
    fn of_published(approver: &PublishedApprover) -> Self {
        match approver {
            PublishedApprover::AwsOrganizationAdmin => Self::AwsOrganizationAdmin {
                organization_id: BUYER_ORGANIZATION_ID.to_string(),
            },
            PublishedApprover::Email { address, name } => Self::Email {
                address: Sensitive::new((*address).to_string()),
                name: Some(Sensitive::new((*name).to_string())),
            },
            PublishedApprover::ProcurementTool { name } => Self::ProcurementTool {
                name: Sensitive::new((*name).to_string()),
            },
        }
    }

    /// Whether this is the approver an organization administrator decides AS,
    /// rather than one it can only decide on behalf of.
    fn is_organization_admin(&self) -> bool {
        matches!(self, Self::AwsOrganizationAdmin { .. })
    }
}

/// What one approver of a started approval has decided, as this emulator persists
/// it.
#[derive(Debug, Clone)]
pub struct ApproverOutcomeRecord {
    /// `^aprout-[A-Za-z0-9-]+$`, minted when the approval was started so a later
    /// call can name this outcome to decide on behalf of its approver.
    pub id: String,
    /// Who the policy named as this approver.
    pub approver: ApproverIdentity,
    /// One of `ApproverOutcomeStatus`'s values.
    pub status: String,
    /// "The note" the decision carried, and `None` while there has been none.
    /// `AgreementApprovalNote` is `@sensitive`, so the record holds it where a
    /// formatter cannot print it ([`Sensitive`]).
    pub note: Option<Sensitive<String>>,
    /// When this outcome last moved.
    pub updated_at: SystemTime,
    /// The purchase order references the decision carried.
    pub purchase_orders: Vec<AgreementApprovalPurchaseOrder>,
    /// The principal that decided on this approver's behalf, and `None` for an
    /// outcome its own approver decided — `CallerIdentity$principalArn`, "the
    /// identity that overrode the approver's outcome".
    pub overridden_by: Option<String>,
}

impl ApproverOutcomeRecord {
    /// Whether this approver can still decide: their level is the one awaiting a
    /// decision.
    fn is_actionable(&self) -> bool {
        self.status == OUTCOME_PENDING
    }

    /// Whether this approver has not decided and could still be asked to.
    fn is_open(&self) -> bool {
        self.status == OUTCOME_PENDING || self.status == OUTCOME_QUEUED
    }

    /// This outcome as the model reports it: the union member its approver's kind
    /// names, carrying the decision this record holds.
    fn to_report(&self) -> ApproverOutcome {
        let purchase_orders =
            (!self.purchase_orders.is_empty()).then(|| self.purchase_orders.clone());
        let note = self.note.as_ref().map(|note| note.get().clone());
        let overridden_by = self
            .overridden_by
            .as_ref()
            .map(|principal_arn| CallerIdentity {
                principal_arn: principal_arn.clone(),
            });
        match &self.approver {
            ApproverIdentity::AwsOrganizationAdmin { .. } => {
                ApproverOutcome::awsOrganizationApproverOutcome(AwsOrganizationApproverOutcome {
                    note,
                    status: self.status.clone(),
                    updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
                    approver_outcome_id: Some(self.id.clone()),
                    purchase_orders,
                })
            }
            ApproverIdentity::Email { address, name } => {
                ApproverOutcome::emailApproverOutcome(EmailApproverOutcome {
                    note,
                    status: self.status.clone(),
                    updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
                    approver_outcome_id: Some(self.id.clone()),
                    email_address: address.get().clone(),
                    name: name.as_ref().map(|name| name.get().clone()),
                    purchase_orders,
                    overridden_by,
                })
            }
            ApproverIdentity::ProcurementTool { name } => {
                ApproverOutcome::procurementToolApproverOutcome(ProcurementToolApproverOutcome {
                    note,
                    status: self.status.clone(),
                    updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
                    approver_outcome_id: Some(self.id.clone()),
                    name: name.get().clone(),
                    purchase_orders,
                    overridden_by,
                })
            }
        }
    }
}

/// One level of a started approval: the approvers at it, how many of them have to
/// approve, and what each of them has decided.
#[derive(Debug, Clone)]
pub struct ApprovalLevel {
    /// The name the published plan gave the level.
    pub name: String,
    /// How many approvers of the level have to approve for it to be approved.
    pub required_approval_count: i32,
    /// The approvers of the level's one group, in the order the policy publishes
    /// them.
    pub approvers: Vec<ApproverOutcomeRecord>,
}

impl ApprovalLevel {
    /// Whether this level has the approvals its criteria require.
    fn is_approved(&self) -> bool {
        let approved = self
            .approvers
            .iter()
            .filter(|approver| approver.status == OUTCOME_APPROVED)
            .count();
        i64::try_from(approved).unwrap_or(i64::MAX) >= i64::from(self.required_approval_count)
    }

    /// This level as the model reports it.
    fn to_report(&self) -> BuyerAgreementApprovalLevel {
        BuyerAgreementApprovalLevel {
            name: Some(self.name.clone()),
            groups: vec![BuyerAgreementApprovalGroup {
                approval_criteria: BuyerAgreementApprovalCriteria {
                    required_approval_count: self.required_approval_count,
                },
                approvers: self
                    .approvers
                    .iter()
                    .map(ApproverOutcomeRecord::to_report)
                    .collect(),
            }],
        }
    }
}

/// Why a decision on an approval was refused — the condition, which each caller
/// states in its own modelled envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionRefusal {
    /// The approval is no longer `PENDING_APPROVAL`, so there is nothing left to
    /// decide (`CLOSED_AGREEMENT_APPROVAL`).
    Closed,
    /// The outcome named is at a level the approval has not reached, whose
    /// approvers "can act only after the previous level is approved"
    /// (`QUEUED_AGREEMENT_APPROVAL`).
    Queued,
    /// The outcome named is no outcome of this approval, or one that has already
    /// been decided (`INVALID_APPROVER_OUTCOME_ID_TO_OVERRIDE`).
    UnknownOutcome,
    /// The caller named no outcome and the level awaiting a decision has no
    /// organization-administrator approver, so the caller is not an approver of
    /// it.
    NotAnApprover,
}

/// One agreement approval, as this emulator persists it.
///
/// `StartAgreementApprovals` mints it from a published plan
/// ([`PublishedApprovalPlan`]) and every later report of it
/// (`GetAgreementApproval`, `ListAgreementApprovals`) is computed from this record
/// and the agreement request it is for.
#[derive(Debug, Clone)]
pub struct AgreementApproval {
    /// `^apr-[A-Za-z0-9]+$`, minted by
    /// [`ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State::mint_agreement_approval_id`].
    pub id: String,
    /// The agreement request this approval is for.
    pub agreement_request_id: String,
    /// The catalog of that request, which `ListAgreementApprovals` filters on.
    pub catalog: String,
    /// "The descriptive name of the agreement approval": the name of the plan it
    /// was started for.
    pub name: String,
    /// The plan this approval was started for
    /// (`BuyerAgreementApprovalPlanInput$planReference`).
    pub plan_reference: String,
    /// "The note of the requester", as the start call carried it.
    /// `AgreementApprovalNote` is `@sensitive`, so the record holds it where a
    /// formatter cannot print it ([`Sensitive`]).
    pub requester_note: Option<Sensitive<String>>,
    /// The account that asked for the approvals.
    pub requester_account_id: String,
    /// One of `AgreementApprovalStatus`'s values.
    pub status: String,
    /// The levels of the plan, in the order they are evaluated.
    pub levels: Vec<ApprovalLevel>,
    /// The purchase order references in force for this approval: the ones the
    /// start call carried, as the decisions on it replace them.
    pub purchase_orders: Vec<AgreementApprovalPurchaseOrder>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// "When approvers could start acting on the agreement approval" — the
    /// instant it was started, approvals of this service being startable at once.
    pub started_at: SystemTime,
    /// "The date and time by which the agreement approval must be completed",
    /// which is also when it "expires if it isn't approved or rejected": one
    /// deadline, [`APPROVAL_WINDOW`] after it was started.
    pub approval_needed_by: SystemTime,
}

impl AgreementApproval {
    /// Start an approval of `request` for `plan`, as the policy publishes it.
    ///
    /// Every approver of the first level is `PENDING_APPROVAL` — the level
    /// approvers can act on at once — and every approver of a later level is
    /// `QUEUED`, since "approvers at a level can act only after the previous level
    /// is approved".
    pub fn start(
        id: String,
        plan: &PublishedApprovalPlan,
        request: &AgreementRequest,
        plan_input: &BuyerAgreementApprovalPlanInput,
        purchase_orders: Vec<AgreementApprovalPurchaseOrder>,
        outcome_ids: &mut impl FnMut() -> String,
        now: SystemTime,
    ) -> Self {
        let levels = plan
            .levels
            .iter()
            .enumerate()
            .map(|(index, level)| ApprovalLevel {
                name: level.name.to_string(),
                required_approval_count: level.required_approval_count,
                approvers: level
                    .approvers
                    .iter()
                    .map(|approver| ApproverOutcomeRecord {
                        id: outcome_ids(),
                        approver: ApproverIdentity::of_published(approver),
                        status: if index == 0 { OUTCOME_PENDING } else { OUTCOME_QUEUED }
                            .to_string(),
                        note: None,
                        updated_at: now,
                        purchase_orders: Vec::new(),
                        overridden_by: None,
                    })
                    .collect(),
            })
            .collect();
        Self {
            id,
            agreement_request_id: request.id.clone(),
            catalog: request.catalog.clone(),
            name: plan.name.to_string(),
            plan_reference: plan.reference.to_string(),
            requester_note: plan_input.requester_note.clone().map(Sensitive::new),
            requester_account_id: ::engine::emulator_account_id().to_string(),
            status: APPROVAL_STATUS_PENDING.to_string(),
            levels,
            purchase_orders,
            created_at: now,
            updated_at: now,
            started_at: now,
            approval_needed_by: now
                .checked_add(APPROVAL_WINDOW)
                .unwrap_or(now),
        }
    }

    /// Whether a decision can still be recorded on this approval.
    pub fn is_open(&self) -> bool {
        self.status == APPROVAL_STATUS_PENDING
    }

    /// The level awaiting a decision: the first one that is not approved yet.
    fn awaiting_level(&self) -> Option<usize> {
        self.levels
            .iter()
            .position(|level| !level.is_approved())
    }

    /// The approver at `approver_index` of the level at `level_index`, or `None`
    /// when this approval has no such approver.
    fn approver_at(
        &self,
        level_index: usize,
        approver_index: usize,
    ) -> Option<&ApproverOutcomeRecord> {
        self.levels
            .get(level_index)?
            .approvers
            .get(approver_index)
    }

    /// Materialise what the CLOCK already owes this approval at `now`: an
    /// approval still awaiting a decision past its own deadline expired at that
    /// deadline, whether or not anybody called.
    ///
    /// Answers whether the approval moved, so a caller can audit the transition
    /// the clock made. Every read of an approval goes through it, so the status
    /// reported is the status the deadline left it in rather than one a later call
    /// happens to notice.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if !self.is_open() || now < self.approval_needed_by {
            return false;
        }
        self.status = APPROVAL_STATUS_EXPIRED.to_string();
        let expired_at = self.approval_needed_by;
        for level in &mut self.levels {
            for approver in &mut level.approvers {
                if approver.is_open() {
                    approver.status = OUTCOME_EXPIRED.to_string();
                    approver.updated_at = expired_at;
                }
            }
        }
        self.updated_at = expired_at;
        true
    }

    /// Cancel this approval because the agreement request it is for was
    /// cancelled: "its open agreement approvals are cancelled".
    ///
    /// Answers whether the approval moved, so only an approval that was open is
    /// audited as cancelled.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if !self.is_open() {
            return false;
        }
        self.status = APPROVAL_STATUS_CANCELLED.to_string();
        for level in &mut self.levels {
            for approver in &mut level.approvers {
                if approver.is_open() {
                    approver.status = OUTCOME_SKIPPED.to_string();
                    approver.updated_at = now;
                }
            }
        }
        self.updated_at = now;
        true
    }

    /// Record a decision on this approval and move everything that decision moves.
    ///
    /// `outcome` names the approver to decide for — the
    /// `approverOutcomeIdToOverride` of the request, which is how an organization
    /// administrator decides "on behalf of another approver" — and `None` means
    /// the administrator decides as the organization-administrator approver of the
    /// level awaiting a decision.
    ///
    /// On an approval: the approver's outcome carries the decision, its note and
    /// the purchase orders it supplied. An approved level skips the approvers of it
    /// whose decision is no longer needed and releases the next level's approvers,
    /// and an approval whose last level is approved is `APPROVED` — "once every
    /// level is approved, the agreement approval transitions to `APPROVED`
    /// status". A rejection closes the approval at once — "once rejected, the
    /// agreement approval and its agreement request transition to `REJECTED`
    /// status" — and the approvers who were still to decide are skipped.
    pub fn decide(
        &mut self,
        outcome: Option<&str>,
        approve: bool,
        note: Option<String>,
        purchase_orders: Vec<AgreementApprovalPurchaseOrder>,
        principal_arn: &str,
        now: SystemTime,
    ) -> Result<(), DecisionRefusal> {
        if !self.is_open() {
            return Err(DecisionRefusal::Closed);
        }
        let awaiting = self.awaiting_level().ok_or(DecisionRefusal::Closed)?;
        // WHICH approver this decision is about, resolved to a position in this
        // record rather than to anything a caller supplied: an
        // `approverOutcomeIdToOverride` is looked UP among the outcomes this
        // approval holds, and a call that names none decides as the
        // organization-administrator approver of the level awaiting a decision.
        // Every lookup below answers a refusal rather than indexing blind, so a
        // request can reach no position this record does not have.
        let (level_index, approver_index) = match outcome {
            Some(id) => {
                let (level_index, approver_index) = self
                    .levels
                    .iter()
                    .enumerate()
                    .find_map(|(level_index, level)| {
                        level
                            .approvers
                            .iter()
                            .position(|approver| approver.id == id)
                            .map(|approver_index| (level_index, approver_index))
                    })
                    .ok_or(DecisionRefusal::UnknownOutcome)?;
                let approver = self
                    .approver_at(level_index, approver_index)
                    .ok_or(DecisionRefusal::UnknownOutcome)?;
                if approver.status == OUTCOME_QUEUED {
                    return Err(DecisionRefusal::Queued);
                }
                if !approver.is_actionable() {
                    return Err(DecisionRefusal::UnknownOutcome);
                }
                (level_index, approver_index)
            }
            None => {
                let approver_index = self
                    .levels
                    .get(awaiting)
                    .ok_or(DecisionRefusal::Closed)?
                    .approvers
                    .iter()
                    .position(|approver| {
                        approver.approver.is_organization_admin() && approver.is_actionable()
                    })
                    .ok_or(DecisionRefusal::NotAnApprover)?;
                (awaiting, approver_index)
            }
        };

        // The decision itself, on the one approver it is about. An administrator
        // deciding for ANOTHER approver is what `overriddenBy` records; one
        // deciding as the organization-administrator approver overrides nobody.
        let overridden = outcome.is_some()
            && !self
                .approver_at(level_index, approver_index)
                .ok_or(DecisionRefusal::UnknownOutcome)?
                .approver
                .is_organization_admin();
        let approver = self
            .levels
            .get_mut(level_index)
            .and_then(|level| level.approvers.get_mut(approver_index))
            .ok_or(DecisionRefusal::UnknownOutcome)?;
        approver.status = if approve { OUTCOME_APPROVED } else { OUTCOME_REJECTED }.to_string();
        approver.note = note.map(Sensitive::new);
        approver.updated_at = now;
        if !purchase_orders.is_empty() {
            approver.purchase_orders = purchase_orders.clone();
        }
        approver.overridden_by = overridden.then(|| principal_arn.to_string());

        // The purchase orders in force on the approval: the references a decision
        // supplied REPLACE the one recorded for the charge each of them names, a
        // charge carrying one reference.
        for supplied in purchase_orders {
            match self
                .purchase_orders
                .iter_mut()
                .find(|held| held.charge_id == supplied.charge_id)
            {
                Some(held) => *held = supplied,
                None => self.purchase_orders.push(supplied),
            }
        }

        if approve {
            // Whether the level this decision was recorded at now has the
            // approvals its criteria require, read back from the record.
            let level_approved = self
                .levels
                .get(level_index)
                .is_some_and(ApprovalLevel::is_approved);
            if level_approved {
                // The approvals the criteria require are in: nobody else at this
                // level has to decide, and the next level's approvers can act.
                if let Some(level) = self.levels.get_mut(level_index) {
                    for approver in &mut level.approvers {
                        if approver.is_open() {
                            approver.status = OUTCOME_SKIPPED.to_string();
                            approver.updated_at = now;
                        }
                    }
                }
                match self.levels.get_mut(level_index.saturating_add(1)) {
                    Some(next) => {
                        for approver in &mut next.approvers {
                            if approver.status == OUTCOME_QUEUED {
                                approver.status = OUTCOME_PENDING.to_string();
                                approver.updated_at = now;
                            }
                        }
                    }
                    None => self.status = APPROVAL_STATUS_APPROVED.to_string(),
                }
            }
        } else {
            self.status = APPROVAL_STATUS_REJECTED.to_string();
            for level in &mut self.levels {
                for approver in &mut level.approvers {
                    if approver.is_open() {
                        approver.status = OUTCOME_SKIPPED.to_string();
                        approver.updated_at = now;
                    }
                }
            }
        }
        self.updated_at = now;
        Ok(())
    }

    /// This approval as `ListAgreementApprovals` and `StartAgreementApprovals`
    /// report it.
    pub fn to_summary(&self) -> AgreementApprovalSummary {
        AgreementApprovalSummary {
            approval_type: APPROVAL_TYPE_BUYER.to_string(),
            agreement_request_id: self.agreement_request_id.clone(),
            agreement_approval_id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
            started_at: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            expires_at: Some(::aws_smithy_types::DateTime::from(self.approval_needed_by)),
            approval_needed_by: ::aws_smithy_types::DateTime::from(self.approval_needed_by),
        }
    }

    /// This approval as `GetAgreementApproval` reports it: the record, plus what
    /// the agreement request it is for says about the proposer and the proposal.
    pub fn to_report(&self, request: &AgreementRequest) -> BuyerAgreementApproval {
        let summary = self.to_summary();
        BuyerAgreementApproval {
            approval_type: summary.approval_type,
            agreement_approval_id: summary.agreement_approval_id,
            name: summary.name,
            agreement_request_id: summary.agreement_request_id,
            status: summary.status,
            requester: AgreementApprovalRequester {
                account_id: self.requester_account_id.clone(),
            },
            created_at: summary.created_at,
            updated_at: summary.updated_at,
            started_at: summary.started_at,
            expires_at: summary.expires_at,
            approval_needed_by: summary.approval_needed_by,
            levels: self.levels.iter().map(ApprovalLevel::to_report).collect(),
            plan_input: BuyerAgreementApprovalPlanInput {
                plan_reference: self.plan_reference.clone(),
                requester_note: self
                    .requester_note
                    .as_ref()
                    .map(|note| note.get().clone()),
            },
            proposer: approval_proposer(),
            proposal_summary: approval_proposal_summary(request),
        }
    }
}

/// The proposer of an agreement approval: the seller's account and the legal name
/// the published offer carries for it.
///
/// `AgreementApprovalProposer` requires both, and the account is the one
/// [`agreement_parties`] names the proposer of every agreement of this emulator
/// by.
fn approval_proposer() -> AgreementApprovalProposer {
    let (_, proposer) = agreement_parties();
    AgreementApprovalProposer {
        account_id: proposer.account_id.unwrap_or_default(),
        legal_name: OFFER_DOCUMENT.proposer_legal_name.to_string(),
    }
}

/// The proposal an approval reports, read from the agreement request it is for.
///
/// `AgreementApprovalResource` requires a `name` for every resource it lists, and
/// the name of a product is published by the catalog the offer belongs to
/// ([`OFFER_RESOURCES`]) rather than carried on the agreement. So the member is
/// reported only when every resource of the proposal is one that catalog publishes
/// a name for, and is left absent — it is optional — rather than naming a product
/// this emulator cannot name.
fn approval_proposal_summary(request: &AgreementRequest) -> AgreementApprovalProposalSummary {
    let resources = request
        .proposal_summary
        .resources
        .iter()
        .flatten()
        .map(|resource| {
            let id = resource.id.clone().unwrap_or_default();
            let name = published_product_name(&id)?;
            Some(AgreementApprovalResource {
                id,
                r#type: resource.r#type.clone().unwrap_or_default(),
                name: name.to_string(),
                // "Whether the resource is deployed on AWS" is a property of the
                // listing rather than of the agreement, and the catalog this
                // emulator publishes carries none, so the member stays absent.
                deployed_on_aws: None,
            })
        })
        .collect::<Option<Vec<AgreementApprovalResource>>>();
    AgreementApprovalProposalSummary {
        offer_id: request
            .proposal_summary
            .offer_id
            .clone()
            .unwrap_or_default(),
        offer_set_id: request.proposal_summary.offer_set_id.clone(),
        resources,
        visibility_scope: OFFER_DOCUMENT.visibility_scope.to_string(),
    }
}

/// The `ValidationExceptionReason` for a start call naming one approval plan
/// twice: an approval is started per plan, so two inputs naming one plan ask for
/// two approvals of the same plan.
pub const REASON_DUPLICATE_PLAN_REFERENCE: &str = "DUPLICATE_PLAN_REFERENCE";

/// The request member a start call names its plans with.
pub const PLAN_INPUTS_MEMBER: &str = "planInputs";

/// Why the approvals of `request` cannot be started in the state it is in, or
/// `None` when they can.
///
/// Approvals are started on a request that has not been submitted for approval
/// yet — `AgreementRequestStatus` `DRAFT`, "created but not yet submitted for
/// approval" — and starting them is what moves it to `PENDING_APPROVAL`
/// ("once the agreement approvals start, the agreement request transitions to
/// `PENDING_APPROVAL` status"). One already waiting for its approvers has its
/// approvals running, and one that has been approved, accepted, rejected or
/// cancelled is closed to new approvals.
pub fn start_refusal_reason(request: &AgreementRequest) -> Option<&'static str> {
    match request.status.as_str() {
        REQUEST_STATUS_DRAFT => None,
        REQUEST_STATUS_PENDING_APPROVAL => Some(REASON_PENDING_APPROVAL_AGREEMENT_REQUEST),
        _ => Some(REASON_CLOSED_AGREEMENT_REQUEST),
    }
}

/// Why a decision on an approval was refused, as the caller states it: the
/// modelled body each condition is answered with.
#[derive(Debug)]
pub enum DecisionRefusalBody {
    /// A fault in the request: the approval is closed, the approver named is
    /// queued, or the outcome named is no outcome this call can decide.
    Validation(ValidationException),
    /// The caller is neither the organization-administrator approver of the level
    /// awaiting a decision nor deciding on behalf of one of its approvers.
    AccessDenied(AccessDeniedException),
}

/// `AccessDeniedException`'s own documentation, which is the sentence this
/// service states when a caller may not perform the decision it asked for
/// (`smithy_query(query="shape", shape="AccessDeniedException")`).
pub const ACCESS_DENIED_MESSAGE: &str =
    "User does not have sufficient access to perform this action.";

/// The `AccessDeniedExceptionReason` for a caller whose access to the decision it
/// asked for is not the access that decision takes.
pub const REASON_INVALID_ACCESS: &str = "INVALID_ACCESS";

/// The `ValidationExceptionReason` for a decision on an approval that is no
/// longer `PENDING_APPROVAL`.
pub const REASON_CLOSED_AGREEMENT_APPROVAL: &str = "CLOSED_AGREEMENT_APPROVAL";
/// The `ValidationExceptionReason` for a decision for an approver whose level the
/// approval has not reached.
pub const REASON_QUEUED_AGREEMENT_APPROVAL: &str = "QUEUED_AGREEMENT_APPROVAL";
/// The `ValidationExceptionReason` for an `approverOutcomeIdToOverride` that
/// names no outcome this call can decide.
pub const REASON_INVALID_APPROVER_OUTCOME: &str = "INVALID_APPROVER_OUTCOME_ID_TO_OVERRIDE";

/// The member of a decision call that names the outcome to decide on behalf of.
pub const APPROVER_OUTCOME_OVERRIDE_MEMBER: &str = "approverOutcomeIdToOverride";

/// The modelled body `refusal` is answered with, where `input_member` is the
/// request member carrying the decision (`buyerAgreementApprovalApproverInput`
/// for an approval, `buyerAgreementApprovalRejectionInput` for a rejection).
///
/// The ONE place the two decision operations state these conditions, so
/// approving and rejecting cannot disagree on the class, the reason or the member
/// for the same condition.
pub fn decision_refusal_body(
    refusal: DecisionRefusal,
    input_member: &str,
) -> DecisionRefusalBody {
    let member_path = |member: &str| {
        ::aws_smithy_schema_server::constraints::path::join(&[
            ::aws_smithy_schema_server::constraints::path::member(input_member),
            ::aws_smithy_schema_server::constraints::path::member(member),
        ])
    };
    match refusal {
        DecisionRefusal::Closed => DecisionRefusalBody::Validation(validation_exception(
            REASON_CLOSED_AGREEMENT_APPROVAL,
            member_path("agreementApprovalId"),
        )),
        DecisionRefusal::Queued => DecisionRefusalBody::Validation(validation_exception(
            REASON_QUEUED_AGREEMENT_APPROVAL,
            member_path(APPROVER_OUTCOME_OVERRIDE_MEMBER),
        )),
        DecisionRefusal::UnknownOutcome => {
            DecisionRefusalBody::Validation(unresolved_reference(
                REASON_INVALID_APPROVER_OUTCOME,
                member_path(APPROVER_OUTCOME_OVERRIDE_MEMBER),
            ))
        }
        // "Allows the management account or a delegated administrator of the
        // buyer's AWS organization to approve … as the organization administrator
        // approver or on behalf of another approver": a caller that is neither
        // has no access to this decision, which is the class
        // `AccessDeniedException` carries, under the reason the model publishes
        // for it.
        DecisionRefusal::NotAnApprover => {
            DecisionRefusalBody::AccessDenied(AccessDeniedException {
                request_id: Some(next_request_id()),
                message: Some(ACCESS_DENIED_MESSAGE.to_string()),
                reason: Some(REASON_INVALID_ACCESS.to_string()),
            })
        }
    }
}

/// The `InternalServerException` for an approval whose agreement request this
/// store no longer holds.
///
/// Every report of an approval states the proposer and the proposal of the
/// request it is for, which are read from that request: an approval cannot be
/// reported without it. Nothing deletes an agreement request, so this names an
/// inconsistent store rather than anything a caller did.
pub fn approval_request_missing(approval_id: &str, request_id: &str) -> InternalServerException {
    InternalServerException {
        request_id: Some(next_request_id()),
        message: Some(format!(
            "agreement approval {approval_id} is for agreement request {request_id}, \
             which this store no longer holds"
        )),
    }
}

/// The `ResourceNotFoundException` for an agreement approval id that names none.
pub fn agreement_approval_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement approval")),
        resource_id: Some(id.to_string()),
        resource_type: Some(RESOURCE_TYPE_AGREEMENT_APPROVAL.to_string()),
    }
}

/// The `ResourceNotFoundException` for a plan reference that names no approval
/// plan of the agreement request it was passed with.
pub fn approval_plan_not_found(reference: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        request_id: Some(next_request_id()),
        message: Some(does_not_exist("agreement request approval plan")),
        resource_id: Some(reference.to_string()),
        resource_type: Some(RESOURCE_TYPE_APPROVAL_PLAN.to_string()),
    }
}

impl ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State {
    /// Materialise what the CLOCK already owes every approval this store holds at
    /// `now`, answering the ids of the approvals that moved.
    ///
    /// Every operation that reads or decides an approval runs it first, for the
    /// reason the agreement settling walk is run first: an approval whose deadline
    /// has passed expired at that deadline whether or not anybody called, so the
    /// status reported is the one the clock left rather than one the next call
    /// happens to notice.
    pub fn settle_agreement_approvals(&mut self, now: SystemTime) -> Vec<String> {
        self.agreement_approvals
            .values_mut()
            .filter_map(|approval| approval.settle(now).then(|| approval.id.clone()))
            .collect()
    }

    /// The approvals started for the agreement request `request_id`, in the order
    /// they were started.
    pub fn approvals_of(&self, request_id: &str) -> Vec<&AgreementApproval> {
        self.agreement_approvals
            .values()
            .filter(|approval| approval.agreement_request_id == request_id)
            .collect()
    }

    /// Move the agreement request `request_id` to the status its approvals now
    /// put it in, answering that status when it moved.
    ///
    /// A request waiting for its approvers is `REJECTED` once one of them rejects
    /// — "once rejected, the agreement approval and its agreement request
    /// transition to `REJECTED` status" — and `APPROVED` once every approval
    /// started for it is approved AND, where the published policy makes approval
    /// of it mandatory, one of the approved approvals is for a plan the policy
    /// makes mandatory: that is when "all required approvals have been granted".
    /// Nothing else moves it: a request whose approvals are still open keeps
    /// waiting.
    pub fn refresh_request_status(
        &mut self,
        request_id: &str,
        now: SystemTime,
    ) -> Option<&'static str> {
        let approvals = self.approvals_of(request_id);
        if approvals.is_empty() {
            return None;
        }
        let rejected = approvals
            .iter()
            .any(|approval| approval.status == APPROVAL_STATUS_REJECTED);
        let approved = approvals
            .iter()
            .all(|approval| approval.status == APPROVAL_STATUS_APPROVED);
        let request = self.agreement_requests.get(request_id)?;
        let value = committed_units(&request.charge_summary);
        let required = request.approval_requirement == APPROVAL_REQUIRED;
        let mandatory_granted = approvals.iter().any(|approval| {
            approval.status == APPROVAL_STATUS_APPROVED
                && applying_plan(value, &approval.plan_reference)
                    .is_some_and(|plan| plan.mandatory)
        });
        let status = if rejected {
            REQUEST_STATUS_REJECTED
        } else if approved && (!required || mandatory_granted) {
            REQUEST_STATUS_APPROVED
        } else {
            return None;
        };
        let request = self.agreement_requests.get_mut(request_id)?;
        if request.status != REQUEST_STATUS_PENDING_APPROVAL {
            return None;
        }
        request.status = status.to_string();
        request.updated_at = now;
        Some(status)
    }
}
