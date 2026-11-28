// SPDX-License-Identifier: CC0-1.0

//! The stored form of a channel contract, and everything only the channel-contract
//! operations need.
//!
//! A channel contract is a signed agreement about a relationship: it names the
//! relationship and its program management account, and carries the type-specific
//! attestation the signatory signs. The create operation is the only writer, so every
//! response that reports a contract — the create's own, `GetChannelContract`,
//! `ListChannelContracts` and `CancelChannelContract` — is built from a record held
//! here.

use std::collections::HashMap;
use std::time::Duration;

use crate::ops::types::{
    Address, ChannelContractDetail, ChannelContractDetails, ChannelContractSummary,
    ChannelContractSummaryDetails, ManagedServicesAttestationDetails,
    ManagedServicesAttestationSummaryDetails, Tag,
};

/// The `ChannelContractId` prefix, from the pattern `^cc-[a-z0-9]{13}$`.
pub const CHANNEL_CONTRACT_ID_PREFIX: &str = "cc-";

/// The `ChannelContractStatus` values this service's writers move a contract
/// between, from the model's value set
/// `PENDING | ACCEPTED | EXPIRED | CANCELED | INACTIVE`.
///
/// A created contract is sent to the signatory for signature, so it starts
/// `PENDING`; a signature makes it `ACCEPTED` and a cancellation `CANCELED`.
/// `EXPIRED` and `INACTIVE` have no published window or trigger, so neither is
/// invented here.
pub const CONTRACT_STATUS_PENDING: &str = "PENDING";
pub const CONTRACT_STATUS_ACCEPTED: &str = "ACCEPTED";
pub const CONTRACT_STATUS_CANCELED: &str = "CANCELED";

/// The catalog in which `CreateChannelContract` needs no signature: its
/// documentation states that in the Sandbox catalog the channel contract is
/// accepted automatically, so a contract created there is `ACCEPTED` as it is
/// stored and every other catalog leaves it `PENDING`.
pub const AUTO_ACCEPT_CATALOG: &str = "Sandbox";

/// How long after it was created a pending channel contract can still be
/// canceled, from `CancelChannelContract`'s documented scope ("a pending channel
/// contract created within the last 30 days").
pub const CANCELLATION_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How many channel contracts this store holds before `CreateChannelContract`
/// answers the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN bound: the real quota is in neither the model nor the operation docs,
/// and this store lives in memory, so the count is capped rather than left to
/// grow with every request.
pub const MAX_CHANNEL_CONTRACTS: usize = 100;

/// The `quotaCode` and `resourceType` the modelled errors carry for a channel
/// contract. The model types both as plain required strings and documents no value
/// set, so the quota is named after the resource it bounds and the resource type
/// is the model's own shape name.
pub const CHANNEL_CONTRACT_QUOTA_CODE: &str = "ChannelContractsPerAccount";
pub const CHANNEL_CONTRACT_RESOURCE_TYPE: &str = "ChannelContract";

/// The ARN of channel contract `id` in `catalog`, in the form the model's
/// `ChannelContractIdentifier` pattern requires
/// (`arn:aws:partnercentral:<region>:<account>:catalog/<catalog>/channel-contract/<id>`).
/// The account segment is the emulator's canonical placeholder account, built
/// through the engine's accessor rather than a literal.
pub fn channel_contract_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        super::ARN_NAMESPACE,
        region,
        &format!("catalog/{catalog}/channel-contract/{id}"),
    )
}

/// The managed-services attestation a `MANAGED_SERVICES_ATTESTATION` contract
/// carries, as stored: the request's own attestation, with each identifier member
/// resolved to the resource it names so a report can be built from the store.
#[derive(Debug, Clone)]
pub struct ManagedServicesAttestation {
    pub legal_entity_name: String,
    /// The bare `rs-…` id of the relationship the attestation is about.
    pub relationship_id: String,
    /// That relationship's ARN, the fully-qualified form of the same identifier.
    pub relationship_arn: String,
    /// The bare `pma-…` id of the relationship's program management account.
    pub program_management_account_id: String,
    /// That account's ARN.
    pub program_management_account_arn: String,
    pub member_account_ids: Option<Vec<String>>,
    pub end_date: ::aws_smithy_types::DateTime,
    pub supported_services: Vec<String>,
    pub legal_entity_address: Address,
    pub signatory_name: String,
    pub signatory_email: String,
}

/// A channel contract as stored. Every response that reports one is built from
/// this, never from the request that created it.
#[derive(Debug, Clone)]
pub struct ChannelContract {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub contract_type: String,
    pub status: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    /// When the signatory responded, absent while the contract is still awaiting
    /// a signature.
    pub responded_at: Option<::aws_smithy_types::DateTime>,
    pub cancellation_reason: Option<String>,
    /// The type-specific attestation. The model's `ChannelContractType` has one
    /// value, so every stored contract carries the managed-services attestation.
    pub attestation: ManagedServicesAttestation,
    pub tags: Vec<Tag>,
}

impl ChannelContract {
    /// This contract as the `ChannelContractDetail` `GetChannelContract` returns.
    pub fn to_detail(&self) -> ChannelContractDetail {
        ChannelContractDetail {
            id: self.id.clone(),
            arn: self.arn.clone(),
            catalog: self.catalog.clone(),
            contract_type: self.contract_type.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            responded_at: self.responded_at,
            cancellation_reason: self.cancellation_reason.clone(),
            details: ChannelContractDetails::managedServicesAttestationDetails(
                ManagedServicesAttestationDetails {
                    legal_entity_name: self.attestation.legal_entity_name.clone(),
                    relationship_identifier: self.attestation.relationship_arn.clone(),
                    program_management_account_identifier: self
                        .attestation
                        .program_management_account_arn
                        .clone(),
                    member_account_ids: self.attestation.member_account_ids.clone(),
                    end_date: self.attestation.end_date,
                    supported_services: self.attestation.supported_services.clone(),
                    signatory_name: self.attestation.signatory_name.clone(),
                    signatory_email: self.attestation.signatory_email.clone(),
                    legal_entity_address: Some(self.attestation.legal_entity_address.clone()),
                },
            ),
        }
    }

    /// This contract as the `ChannelContractSummary` `ListChannelContracts`
    /// returns. The summary's own details shape declares neither the signatory nor
    /// the legal entity address — the model does not carry them on it — so those
    /// are reachable only through `GetChannelContract`.
    pub fn to_summary(&self) -> ChannelContractSummary {
        ChannelContractSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            catalog: self.catalog.clone(),
            contract_type: self.contract_type.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            responded_at: self.responded_at,
            details: ChannelContractSummaryDetails::managedServicesAttestationDetails(
                ManagedServicesAttestationSummaryDetails {
                    legal_entity_name: self.attestation.legal_entity_name.clone(),
                    relationship_identifier: self.attestation.relationship_arn.clone(),
                    program_management_account_identifier: self
                        .attestation
                        .program_management_account_arn
                        .clone(),
                    member_account_ids: self.attestation.member_account_ids.clone(),
                    end_date: self.attestation.end_date,
                    supported_services: self.attestation.supported_services.clone(),
                },
            ),
        }
    }

    /// Whether this contract was created no longer than `window` ago, as of
    /// `now`: the scope `CancelChannelContract` documents for a cancellation.
    pub fn created_within(&self, window: Duration, now: ::aws_smithy_types::DateTime) -> bool {
        let age = now.secs().saturating_sub(self.created_at.secs());
        age <= i64::try_from(window.as_secs()).unwrap_or(i64::MAX)
    }
}

/// The channel contracts of this service, keyed by id and kept in creation order
/// so a paginated list walks a stable sequence.
#[derive(Debug, Default)]
pub struct ChannelContractStore {
    order: Vec<String>,
    by_id: HashMap<String, ChannelContract>,
}

impl ChannelContractStore {
    pub fn insert(&mut self, contract: ChannelContract) {
        self.order.push(contract.id.clone());
        self.by_id.insert(contract.id.clone(), contract);
    }

    pub fn get(&self, id: &str) -> Option<&ChannelContract> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ChannelContract> {
        self.by_id.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// Every stored contract, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ChannelContract> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The filters `ListChannelContracts` accepts, already resolved: each is `None`
/// when the request omitted it (no constraint) and `Some(values)` otherwise.
/// Built by the handler and carried here so the matching rule and the token
/// fingerprint read the same thing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChannelContractFilters {
    pub catalog: String,
    pub contract_type: String,
    pub statuses: Option<Vec<String>>,
    /// From the `managedServicesAttestationTypeFilters` variant of
    /// `contractTypeFilters`, already resolved from identifiers (bare id or ARN)
    /// to bare `rs-…` ids.
    pub relationship_ids: Option<Vec<String>>,
    /// From the same variant's `legalEntityNames`.
    pub legal_entity_names: Option<Vec<String>>,
}

impl ChannelContractFilters {
    /// Does `contract` satisfy every filter?
    ///
    /// The catalog and the contract type scope the list (both are required
    /// members, and a contract has exactly one of each); each remaining filter
    /// matches when the contract's corresponding field equals any of that
    /// filter's values, and several filters are ANDed. An omitted filter
    /// constrains nothing.
    pub fn matches(&self, contract: &ChannelContract) -> bool {
        fn any_equals(values: &Option<Vec<String>>, field: &str) -> bool {
            values
                .as_ref()
                .is_none_or(|values| values.iter().any(|value| value == field))
        }

        contract.catalog == self.catalog
            && contract.contract_type == self.contract_type
            && any_equals(&self.statuses, &contract.status)
            && any_equals(&self.relationship_ids, &contract.attestation.relationship_id)
            && any_equals(
                &self.legal_entity_names,
                &contract.attestation.legal_entity_name,
            )
    }
}
