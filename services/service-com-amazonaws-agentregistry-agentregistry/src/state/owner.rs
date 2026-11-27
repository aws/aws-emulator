// SPDX-License-Identifier: CC0-1.0

//! The store this service's discovery reads report: the one the Agent Registry CONTROL
//! plane owns.
//!
//! This model binds no operation that creates a registry or a registry record —
//! `CreateRegistry`, `CreateRegistryRecord`, `SubmitRegistryRecordForApproval` and
//! `UpdateRegistryRecordStatus` are all bound by
//! `service-com-amazonaws-agentregistrycontrol-agentregistrycontrol` — so the registries
//! and records a discovery read reports are that service's resources, held in ITS store
//! in the shared engine. A second map here would be a second copy of one resource: a
//! record created and approved through the control plane would be invisible to discovery,
//! and a record seeded here would be invisible to `GetRegistryRecord`.
//!
//! So nothing in this crate holds a registry. [`registry`] resolves the registry an
//! identifier names in the owner's store and PROJECTS it into this model's shapes: the two
//! models publish separate (if largely parallel) descriptor structures, and the discovery
//! surface publishes fewer members than the control plane stores — a record's provenance,
//! its tags and a descriptor source's credential-provider configurations are the control
//! plane's and appear in no discovery response — so the projection is written out member by
//! member rather than passed through.

use super::{Registry, RegistryRecord};
use crate::ops::types as discovery;
use crate::ops::types::{InternalServerException, ResourceNotFoundException};
use ::service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::state as control;
use ::service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::ops::types as owner;

/// This engine carries no Agent Registry store at all, so no discovery read can be
/// answered from it. [`resolve`] turns it into the modelled `InternalServerException`
/// worded by [`STORE_UNAVAILABLE_MESSAGE`], which each operation carries in its own error
/// channel; it is never a "that registry does not exist".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreUnavailable;

/// What every discovery read states for [`StoreUnavailable`]: one sentence, in one place,
/// so the three reads cannot word the same server fault differently. It names no part of
/// the request — the condition is a property of the engine, not of what was asked.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the Agent Registry store is unavailable";

/// The registry the `RegistryIdentifier` `identifier` names (a bare registry id or the
/// registry's full ARN), as the discovery surface sees it.
///
/// `Ok(None)` when the owner's store holds no such registry — which is what every
/// discovery read answers `ResourceNotFoundException` for — and `Err(StoreUnavailable)`
/// when this engine carries no store to resolve it in. A discovery read calls
/// [`resolve`], which is where those two failures become the modelled errors it answers
/// them with.
///
/// The owner's clock-owed status transitions are materialised first, through the owner's
/// own accessor: a registry whose creation span has elapsed is `READY` and a record whose
/// creation span has elapsed is `DRAFT` at the same instant whether or not anybody asked,
/// so a read that materialises them reports what a caller would see either way and writes
/// nothing else.
pub fn registry<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
) -> Result<Option<Registry>, StoreUnavailable> {
    let Some(state) = control::service_state(engine) else {
        return Err(StoreUnavailable);
    };
    let mut state = state.lock();
    state.materialise_due_transitions();
    let registry_id = super::registry_id_from_identifier(identifier);
    Ok(state.registries.get(registry_id).map(discoverable))
}

/// How ONE discovery read carries the two ways resolving a registry can fail.
///
/// [`resolve`] is where each failure of [`registry`] becomes a modelled exception, so
/// which exception a failure becomes — and the sentence it carries — is decided in one
/// place for all three reads. What a read still owns is its own error channel: every
/// operation of this model declares its errors as an enum of its own, so each read
/// implements this trait by naming the variant it carries an already-worded exception
/// in, and nothing else.
pub trait RegistryResolution: Sized {
    /// This read's channel for [`StoreUnavailable`]: the modelled
    /// `InternalServerException` worded by [`STORE_UNAVAILABLE_MESSAGE`].
    fn store_unavailable(exception: InternalServerException) -> Self;

    /// This read's channel for a registry the owner's store does not hold: the modelled
    /// `ResourceNotFoundException` worded by [`super::registry_not_found`].
    fn registry_not_found(exception: ResourceNotFoundException) -> Self;
}

/// The registry the `RegistryIdentifier` `identifier` names, or the calling read's own
/// modelled refusal for why it did not resolve — the one mapping of [`registry`]'s two
/// failure modes onto the modelled errors every discovery read answers them with.
///
/// This is what a read calls: it resolves the registry through [`registry`] and leaves
/// the read only the lift into its own error channel ([`RegistryResolution`]), so the
/// three reads cannot drift in which failure they report as which modelled error.
pub fn resolve<Eng: Send + Sync + 'static, Refusal: RegistryResolution>(
    engine: &Eng,
    identifier: &str,
) -> Result<Registry, Refusal> {
    match registry(engine, identifier) {
        Err(StoreUnavailable) => Err(Refusal::store_unavailable(InternalServerException {
            message: Some(STORE_UNAVAILABLE_MESSAGE.to_string()),
        })),
        // The identifier is stated exactly as the request supplied it, which is why the
        // refusal is built from the identifier this resolver was handed.
        Ok(None) => Err(Refusal::registry_not_found(super::registry_not_found(
            identifier,
        ))),
        Ok(Some(registry)) => Ok(registry),
    }
}

/// One registry of the owner's store as the discovery surface sees it: its identity and
/// every record it holds, each projected by [`record`].
///
/// EVERY record is carried over, whatever its status, because which of them a discovery
/// read may report is the reads' own rule ([`RegistryRecord::is_discoverable`]) and not a
/// property of the store.
fn discoverable(held: &control::Registry) -> Registry {
    // Whether this registry declares a custom-metadata schema is the presence condition for
    // `RegistryRecordSummary$customMetadata`, and it is the REGISTRY's property, so it is read
    // here — once per registry, from the registry in hand — and carried onto every record it
    // owns ([`super::RegistryRecord::published_custom_metadata`]).
    let custom_metadata_schema_configured =
        held.custom_metadata_schema_configuration.is_some();
    Registry {
        registry_id: held.registry_id.clone(),
        registry_arn: held.registry_arn.clone(),
        records: held
            .records
            .iter()
            .map(|(record_id, held)| {
                (
                    record_id.clone(),
                    record(held, custom_metadata_schema_configured),
                )
            })
            .collect(),
    }
}

/// One stored record as the discovery surface publishes it.
///
/// What this surface serves is the record's APPROVED REVISION — the content the approval
/// workflow last published ([`control::ApprovedRevision`]) — so a record that has one is
/// projected from it, whatever content the control plane holds for the record now and
/// whatever status the record is in there: an edit withdraws the record from the approval
/// workflow without publishing the edited content, so consumers keep resolving the last
/// approved revision until that edit is approved in turn, and a curation step that takes the
/// record off this surface is what withdraws it. A record with no published revision is
/// projected as it stands, and its own status is what keeps it off the surface
/// ([`super::RegistryRecord::is_discoverable`]).
///
/// `custom_metadata_schema_configured` is the owning registry's answer to whether it declares
/// a custom-metadata schema, which is what decides whether a discovery response carries this
/// record's metadata at all ([`super::RegistryRecord::published_custom_metadata`]).
fn record(
    held: &control::RegistryRecord,
    custom_metadata_schema_configured: bool,
) -> RegistryRecord {
    let published = held.approved_revision.as_ref();
    RegistryRecord {
        record_id: held.record_id.clone(),
        record_arn: held.record_arn.clone(),
        registry_arn: held.registry_arn.clone(),
        name: published.map_or(&held.name, |revision| &revision.name).clone(),
        display_name: published
            .map_or(&held.display_name, |revision| &revision.display_name)
            .clone(),
        description: published
            .map_or(&held.description, |revision| &revision.description)
            .clone(),
        record_type: published
            .map_or(&held.record_type, |revision| &revision.record_type)
            .clone(),
        record_version: published
            .map_or(&held.record_version, |revision| &revision.record_version)
            .clone(),
        // A published revision IS approved content, so that is the status a discovery read
        // reports for it; the record's live status is the control plane's to report.
        status: match published {
            Some(_) => super::STATUS_APPROVED.to_string(),
            None => held.status.clone(),
        },
        descriptors: published
            .map_or(&held.descriptors, |revision| &revision.descriptors)
            .as_ref()
            .map(descriptors)
            .unwrap_or_default(),
        custom_metadata: published
            .map_or(&held.custom_metadata, |revision| &revision.custom_metadata)
            .clone(),
        custom_metadata_schema_configured,
        created_at: held.created_at,
        updated_at: published.map_or(held.updated_at, |revision| revision.updated_at),
    }
}

/// The record's descriptor set, member by member: the two models declare parallel
/// descriptor structures, so each entry is rebuilt in this model's own shape.
fn descriptors(held: &owner::Descriptors) -> discovery::Descriptors {
    discovery::Descriptors {
        mcp_server: held.mcp_server.as_ref().map(|held| discovery::McpServerDescriptor {
            data: held.data.clone(),
            data_schema_version: held.data_schema_version.clone(),
            additional_data: held.additional_data.as_ref().map(|held| {
                discovery::McpServerAdditionalData {
                    tools: held.tools.as_ref().map(|held| discovery::McpToolsDescriptor {
                        data: held.data.clone(),
                        data_schema_version: held.data_schema_version.clone(),
                    }),
                }
            }),
            source: held.source.as_ref().map(source),
        }),
        a2a_agent_card: held
            .a2a_agent_card
            .as_ref()
            .map(|held| discovery::A2aAgentCardDescriptor {
                data: held.data.clone(),
                data_schema_version: held.data_schema_version.clone(),
                source: held.source.as_ref().map(source),
            }),
        agent_skills_definition: held.agent_skills_definition.as_ref().map(|held| {
            discovery::AgentSkillsDefinitionDescriptor {
                data: held.data.clone(),
                data_schema_version: held.data_schema_version.clone(),
                additional_data: held.additional_data.as_ref().map(|held| {
                    discovery::AgentSkillsAdditionalData {
                        skill_md: held.skill_md.as_ref().map(|held| {
                            discovery::AgentSkillsMdDescriptor {
                                data: held.data.clone(),
                                data_schema_version: held.data_schema_version.clone(),
                                source: held.source.as_ref().map(source),
                            }
                        }),
                    }
                }),
            }
        }),
        custom: held
            .custom
            .as_ref()
            .map(|held| discovery::CustomDescriptor {
                data: held.data.clone(),
            }),
        http: held.http.as_ref().map(|held| discovery::HttpDescriptor {
            source: held.source.as_ref().map(source),
        }),
        agui: held.agui.as_ref().map(|held| discovery::AgUiDescriptor {
            source: held.source.as_ref().map(source),
        }),
    }
}

/// Where a descriptor was fetched from, as the discovery surface publishes it: the URL
/// alone. The credential-provider configurations the control plane stores beside it name
/// the credentials used to FETCH the descriptor, and `DescriptorSourceFromUrl` of this
/// model declares no member for them, so no discovery response carries them.
fn source(held: &owner::DescriptorSource) -> discovery::DescriptorSource {
    discovery::DescriptorSource {
        from_url: held
            .from_url
            .as_ref()
            .map(|held| discovery::DescriptorSourceFromUrl {
                url: held.url.clone(),
            }),
    }
}
