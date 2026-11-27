// SPDX-License-Identifier: CC0-1.0

//! The AMAZON COGNITO identity pools this service's authorization reads, which are not this
//! service's own records.
//!
//! An IoT policy can be attached to a Cognito identity, and `GetEffectivePolicies` takes a
//! `cognitoIdentityPoolId` as one of its two identities — but an identity pool is a resource
//! of Amazon Cognito Identity, which HOLDS it: it is created there
//! (`CreateIdentityPool`), described there and deleted there. So whether a pool exists is
//! that service's answer and not a record kept a second time here: this module reads the
//! holder's own store out of the SAME shared engine, exactly as the domain-configuration
//! plane reads the certificate holder's ([`super::acm_certificates`]).
//!
//! A pool the holder does not have is a pool that does not exist, which is what a request
//! naming one is refused for. An engine on which the holder is not mounted at all holds no
//! identity pool either, so it answers the same way — never a pool invented here.

/// The Amazon Cognito Identity store in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// The pools are read from that service's own store in this one shared engine rather than
/// from a copy of its own here: the engine keys state by TYPE, so a copy here would be
/// invisible to the holder and a pool the holder has would address nothing.
///
/// LOCK ORDER: take [`super::service_state`] first and this one second, which is the order
/// every other reader of another service's store takes.
pub fn cognito_identity_pools<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::ComAmazonawsCognitoidentityAwscognitoidentityserviceState,
    >,
> {
    ::service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::service_state(engine)
}

/// Whether the identity pool `identity_pool_id` EXISTS, which is the holder's answer:
/// `true` only when Amazon Cognito Identity holds a pool under that id in this shared
/// engine.
///
/// An engine the holder is not mounted on holds no pool, so every id is unheld there: a
/// reader of this answers that the pool does not exist rather than reporting on one no
/// service has.
pub fn holds_identity_pool<Eng: Send + Sync + 'static>(engine: &Eng, identity_pool_id: &str) -> bool {
    cognito_identity_pools(engine)
        .is_some_and(|pools| pools.lock().identity_pools.contains_key(identity_pool_id))
}

/// How the holder of identity pools states that an `IdentityPoolId` names no pool it has, which
/// is the sentence a reader of its pools refuses such a request with.
///
/// The wording is the HOLDER's own, stated once in its store
/// (`::service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_not_found_message`)
/// and read from there rather than copied here, so a pool this service cannot find and one that
/// service cannot find are refused in one sentence.
pub fn missing_identity_pool_message(identity_pool_id: &str) -> String {
    ::service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::identity_pool_not_found_message(
        identity_pool_id,
    )
}
