// SPDX-License-Identifier: CC0-1.0

//! The capabilities an OpenSearch UI application has registered.
//!
//! A capability is a sub-resource of ONE application — "registers a capability for an
//! OpenSearch UI application […] to enable specific capabilities, such as AI features,
//! for a given application" — addressed by its own name within that application, so the
//! record lives ON the [`Application`] it was registered for ([`Application::capabilities`])
//! the way a data source attachment does. `RegisterCapability` mints it, `GetCapability`
//! reports it and `DeregisterCapability` removes it, all from the ONE [`Capability`]
//! record below, so the answer a register gives and the answer a later read gives cannot
//! disagree.
//!
//! The capability's name is unique per application, so it IS the key the application holds
//! the record under; a register naming a capability the application already holds therefore
//! re-states that one capability rather than adding a second record.

use super::*;

/// `CapabilityStatus`'s `creating` value: the capability has been registered and is
/// being enabled for the application.
pub const CAPABILITY_STATUS_CREATING: &str = "creating";

/// `CapabilityStatus`'s `active` value: the capability is enabled and can be used.
pub const CAPABILITY_STATUS_ACTIVE: &str = "active";

/// `CapabilityStatus`'s `deleting` value: the capability's removal has been asked for
/// and is being carried out — "returns deleting when the capability is being removed"
/// (`DeregisterCapabilityResponse$status`).
pub const CAPABILITY_STATUS_DELETING: &str = "deleting";

/// How many capabilities ONE application holds before a register is refused rather than
/// adding another record.
///
/// Like [`ATTACHMENT_QUOTA`] this is the emulator's OWN bound on the store — the model
/// publishes none — and it is what stops a caller looping with fresh capability names
/// from growing one application's state without end; every record it counts is released
/// when the deregistration that asked for its removal completes
/// ([`Capability::begin_deregistration`]). It bounds the WHOLE set too, because a register
/// resolves its `applicationId` against the applications this account holds and those are
/// bounded by [`APPLICATION_QUOTA`]: no request can partition the store past that product.
pub const CAPABILITY_QUOTA: usize = 25;

/// One registered capability of an OpenSearch UI application, as this emulator persists
/// it.
#[derive(Debug, Clone)]
pub struct Capability {
    /// `CapabilityName`: "the name of the capability […] identifies the type of
    /// capability being enabled for the application". Unique per application, so it IS
    /// the key the application holds the record under, and the value `GetCapability` and
    /// `DeregisterCapability` address it by.
    pub name: String,
    /// "The configuration settings for the capability being registered", as the register
    /// that last stated them carried it. Stored as the modelled union every read path
    /// reports rather than as a second hand-written mirror of its branches.
    pub config: CapabilityBaseRequestConfig,
    /// Where the capability is in its lifecycle: one of `CapabilityStatus`'s values.
    pub status: String,
    /// The status move the clock owes this capability, if any is outstanding.
    pub owed: Option<OwedStatus>,
    /// The instant this capability's record is gone, set by the deregistration that asked
    /// for its removal and absent on one nothing has asked to remove.
    ///
    /// A deregistration does not take the record away where it stands: the capability
    /// rests in its deleting status — which is the status that call reports — and what the
    /// clock owes it is its removal, the same shape as the status moves [`OwedStatus`]
    /// carries, for a move whose outcome is that the application holds no such capability
    /// at all.
    pub removal_due: Option<SystemTime>,
}

impl Capability {
    /// The capability a register mints: it is not enabled the instant the call returns,
    /// so it rests in [`CAPABILITY_STATUS_CREATING`] and the clock owes it the status
    /// that enabling leaves it in one [`PROVISIONING`] span later.
    pub fn registered(name: &str, config: CapabilityBaseRequestConfig, now: SystemTime) -> Self {
        Self {
            name: name.to_string(),
            config,
            status: CAPABILITY_STATUS_CREATING.to_string(),
            owed: Some(OwedStatus {
                status: CAPABILITY_STATUS_ACTIVE.to_string(),
                message: None,
                due: now + PROVISIONING,
            }),
            removal_due: None,
        }
    }

    /// Materialise the status move the clock owes this capability, if `now` is past its
    /// due instant: one that was being enabled becomes one that is active.
    ///
    /// This is the only write a reporting operation makes, and it writes only what was
    /// already owed at the same instant had nobody called.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(owed) = self.owed.take_if(|owed| now >= owed.due) else {
            return;
        };
        self.status = owed.status;
    }

    /// Start this capability's removal, as the call that asked for it leaves the record —
    /// the capability mirror of [`Application::begin_deletion`].
    ///
    /// The capability is not gone the instant that call returns: it is being removed, so
    /// it rests in [`CAPABILITY_STATUS_DELETING`] and the clock owes the record's removal
    /// one [`PROVISIONING`] span later, which [`Application::settle_capabilities`]
    /// performs when it falls due. Whatever status move was outstanding is dropped: a
    /// capability on its way out is on its way to no other status.
    ///
    /// Idempotent — a capability whose removal the clock already owes keeps the instant it
    /// was owed at, so repeating the call cannot push the removal back.
    pub fn begin_deregistration(&mut self, now: SystemTime) {
        if self.removal_due.is_some() {
            return;
        }
        self.owed = None;
        self.status = CAPABILITY_STATUS_DELETING.to_string();
        self.removal_due = Some(now + PROVISIONING);
    }

    /// Whether the removal the clock owed this capability has fallen due by `now`, so the
    /// application no longer holds it.
    pub fn removal_landed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Whether this capability is on its way out, so what the application holds under its
    /// name is a record being removed rather than a registration a caller can use.
    pub fn is_being_removed(&self) -> bool {
        self.removal_due.is_some()
    }

    /// The stored configuration as `RegisterCapability` reports it.
    ///
    /// The register and the response configurations are separate shapes declaring the
    /// SAME branches, so the stored union is projected branch for branch rather than a
    /// second copy of the configuration being kept per shape.
    pub fn to_base_response_config(&self) -> CapabilityBaseResponseConfig {
        match &self.config {
            CapabilityBaseRequestConfig::aiConfig(ai_config) => {
                CapabilityBaseResponseConfig::aiConfig(ai_config.clone())
            }
        }
    }

    /// The stored configuration as `GetCapability` reports it — the extended response
    /// configuration, projected from the SAME stored union
    /// [`Self::to_base_response_config`] projects, so a register's answer and a later read
    /// cannot report one capability's configuration differently.
    pub fn to_extended_response_config(&self) -> CapabilityExtendedResponseConfig {
        match &self.config {
            CapabilityBaseRequestConfig::aiConfig(ai_config) => {
                CapabilityExtendedResponseConfig::aiConfig(ai_config.clone())
            }
        }
    }
}

impl Application {
    /// Materialise everything the clock owes this application's capabilities, for a caller
    /// about to report on any of them — and take the record of every capability whose
    /// removal has fallen due away, so a call about it finds no such capability rather
    /// than one resting in its deleting status for ever.
    pub fn settle_capabilities(&mut self, now: SystemTime) {
        for capability in self.capabilities.values_mut() {
            capability.settle(now);
        }
        self.capabilities
            .retain(|_, capability| !capability.removal_landed(now));
    }
}

impl ComAmazonawsOpensearchAmazonopensearchserviceState {
    /// Materialise everything the clock owes the records a capability operation reads: the
    /// applications the capabilities belong to, and the capabilities' own moves.
    ///
    /// The ONE place the capability operations settle, so the three of them cannot drift
    /// apart over which records they report at `now`.
    pub fn settle_for_capabilities(&mut self, now: SystemTime) {
        self.settle_applications(now);
        for application in self.applications.values_mut() {
            application.settle_capabilities(now);
        }
    }
}

/// The ONE capability name an application's registration may name.
///
/// `CapabilityName` is modelled only as a 3..30-character `^[a-zA-Z0-9-]+$` string, and
/// the service registers exactly one kind of capability — the AI capability
/// `RegisterCapability` is documented to enable, "for registering AI Assistant
/// capability, use ai-capability" — so a name inside the modelled expression that is not
/// this identifier names a capability there is nothing to register
/// ([`capability_name_refusal`]).
pub const AI_CAPABILITY_NAME: &str = "ai-capability";

/// Whether `capability_name` names the capability this service registers
/// ([`AI_CAPABILITY_NAME`]), and the refusal naming the rejected value otherwise.
///
/// The refusal is the client-input one: the value is inside every constraint the model
/// publishes, and what it fails is the service's own rule about WHICH capability can be
/// registered, so it names the value the request carried and the one name that is
/// allowed. Nothing is registered for a name this refuses, so a later read of it answers
/// [`capability_not_found`].
pub fn capability_name_refusal(capability_name: &str) -> Option<ValidationException> {
    (capability_name != AI_CAPABILITY_NAME).then(|| ValidationException {
        message: Some(format!(
            "Invalid capability name for AI Capability: {capability_name}. Only \
             {AI_CAPABILITY_NAME} is allowed"
        )),
    })
}

/// The `ResourceNotFoundException` payload for an application and capability pair the
/// application holds no registration for.
///
/// A capability is a sub-resource of an application, so the refusal names BOTH values the
/// request carried — which is what tells a caller handed two names which one could not be
/// resolved — the way the single-attachment read's refusal does
/// ([`attachment_not_found`]).
pub fn capability_not_found(application_id: &str, capability_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Capability {capability_name} not found for application {application_id}"
        )),
    }
}

/// The `ServiceQuotaExceededException` payload for a register made while the application
/// already holds [`CAPABILITY_QUOTA`] capabilities.
///
/// `RegisterCapability` models that error — "an exception for when a request would cause a
/// service quota to be exceeded" — so the bound on the store is reported through it rather
/// than through the client-input error.
pub fn capability_quota_exceeded(application_id: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Application {application_id} already holds the maximum of {CAPABILITY_QUOTA} \
             capabilities."
        )),
    }
}
