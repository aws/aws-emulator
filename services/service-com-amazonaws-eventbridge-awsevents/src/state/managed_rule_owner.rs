// SPDX-License-Identifier: CC0-1.0

//! WHICH Amazon Web Services service is making the call that writes a rule — the one fact
//! that makes a rule a MANAGED one rather than the customer's own.
//!
//! "If this is a managed rule, created by an Amazon Web Services service on your behalf, this
//! field displays the principal name of the service that created the rule"
//! (`Rule$ManagedBy`). So `ManagedBy` is not a member of any request: it is read off WHO made
//! the call. A service that provisions a rule for one of its own resources — the rule a
//! GuardDuty Malware Protection plan owns for its protected bucket among them — makes that
//! `PutRule` as its own service principal, and the rule it leaves behind carries that
//! principal and is protected from customer writes like every other managed rule
//! ([`super::managed_rule_refusal`]).
//!
//! WHICH credential the call presents is what says so, exactly as it does for every other
//! cross-service call an emulated service makes as its own service principal
//! ([`::service_common::service_principal_calls`], the one directory in the shared engine
//! both sides read): the calling service mints a credential for the one call, records under
//! that credential's access key id the principal it is calling as, and this reads the record
//! back for the access key id the request PRESENTS — the one thing about a request a client
//! cannot choose for itself, resolved by the emulator's own verifier. A request whose
//! credential the directory holds no live record for is a customer's own call, so a client
//! can neither author a managed rule nor pass itself off as the service that owns one.
//!
//! The same reading answers the other half: a managed rule may be written by the service that
//! OWNS it ([`owned_by_the_caller`]), because that service is what maintains it — the rule's
//! pattern and its targets are the owning service's to set. Every other writer is refused.

use ::std::time::SystemTime;

/// The service principal the request these `ctx` came from is being made as, or `None` for a
/// request that is not an emulated service's own call.
///
/// `None` for every customer request, for an engine carrying no such directory, and for a
/// credential whose record the clock has already passed — each of which is a call that
/// provisions nothing on the account's behalf.
///
/// LOCKING: this takes the directory's lock and no other, so a handler may ask it while
/// holding this service's own store guard.
#[must_use]
pub fn provisioning_service<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<String> {
    let directory = in_flight_service_principal_calls(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let held = directory.lock();
    let call = held.call(
        &access_key_id,
        ::engine::emulator_account_id(),
        SystemTime::now(),
    )?;
    Some(call.principal.clone())
}

/// Whether `rule` is a managed rule of the very service making this call, which is the one
/// writer a managed rule admits.
///
/// `false` for a rule that is nobody's managed rule (the customer's own, which this question
/// is not about), for a customer call, and for a service call about ANOTHER service's managed
/// rule.
#[must_use]
pub fn owned_by_the_caller(rule: &super::RuleRecord, provisioning: Option<&str>) -> bool {
    match (rule.managed_by.as_deref(), provisioning) {
        (Some(owner), Some(caller)) => owner == caller,
        _ => false,
    }
}

/// The shared directory of the cross-service calls another service is making as its own
/// service principal, in the one shared engine: written by that service, read here.
fn in_flight_service_principal_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}
