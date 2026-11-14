// SPDX-License-Identifier: CC0-1.0

//! The KMS grant a domain's storage holds on the key that storage is encrypted under — the
//! ONE statement of that rule, so every call that leaves a domain encrypted (or stops it
//! being) records the same thing in KMS.
//!
//! `EncryptionAtRestOptions` is "whether the domain should encrypt data at rest, and if so,
//! the Key Management Service (KMS) key to use"
//! (`smithy_query(query="doc",shape="EncryptionAtRestOptions")`). A domain whose data is
//! encrypted at rest does not use that key as the CALLER that configured it: its nodes ask
//! the key for the data key their storage is encrypted under, and ask it again to decrypt
//! that data key whenever the storage is read, long after the configuring request has
//! returned. The service therefore needs its own permission on the key, and the instrument
//! KMS has for that is a GRANT — "a policy instrument that allows Amazon Web Services
//! principals to use KMS keys in cryptographic operations"
//! (`smithy_query(query="doc",shape="CreateGrant")` in the KMS model) — which is why a grant
//! is the only observable binding a domain's storage to its key: it is what
//! `kms:ListGrants` reports, what a key inventory reads a key's dependants from, and what a
//! client revokes to take the key away from the domain.
//!
//! So the grant follows the configuration:
//!
//! * a domain that is encrypted at rest under a key holds exactly ONE grant on that key,
//!   whichever call left it so — the create that asked for it, or the update that enabled
//!   it;
//! * a domain whose encryption names ANOTHER key holds its grant there instead, and the
//!   grant on the key it no longer uses is retired;
//! * a domain that is not encrypted at rest, and a domain that is gone, hold none.
//!
//! WHO the grant is for is the service itself, under its REGIONAL service principal
//! ([`service_principal`]): the nodes using the key are the service's, in the domain's own
//! Region, and the same principal is the retiring principal, so the service can give the
//! permission back when the domain stops using the key. That principal is not an IAM
//! identity ARN, which is the only form `CreateGrant` takes from a caller
//! (`service_com_amazonaws_kms_trentservice::state::names_a_principal_arn`), so this grant
//! is not one any request could have made: the SERVICE takes it, which is why it is written
//! into the KMS store here rather than through that operation.
//!
//! WHAT it permits is what the service does with the key for that domain and nothing more
//! ([`GRANT_OPERATIONS`]), and WHICH data it permits it for is the domain: the grant carries
//! an encryption-context constraint naming the domain
//! ([`DOMAIN_ARN_CONTEXT_KEY`]), so the permission reaches the data keys of THAT domain
//! only — "these encryption context constraints allow the permissions in the grant only
//! when the encryption context in the request matches … or includes … the encryption
//! context specified in the constraint"
//! (`smithy_query(query="doc",shape="CreateGrantRequest",member="Constraints")`).
//!
//! The grant is written in the KMS service's own store, reached through the SAME shared
//! engine this service's handlers reach their own store through — the store `kms:CreateGrant`
//! writes and `kms:ListGrants`, `kms:ListRetirableGrants` and `kms:RevokeGrant` report and
//! move — the way [`super::encryption_at_rest`] resolves the key itself. An engine handing
//! out no KMS store records nothing: there is no KMS to hold a grant, and a domain is
//! created either way.
//!
//! LOCKING: this takes the KMS store's guard, and a handler may hold this service's own
//! while it does. No KMS operation reads this service's store, so the two can never be taken
//! in opposite orders, and the work done under both is one lookup and one write.

use std::collections::HashMap;
use std::time::SystemTime;

use ::service_com_amazonaws_kms_trentservice::ops::types::GrantConstraints;
use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::ComAmazonawsKmsTrentserviceState as KmsState;

use crate::ops::types::EncryptionAtRestOptions;

/// The encryption-context key the data keys of ONE domain carry, and so the key of the
/// `EncryptionContextSubset` constraint that scopes this grant to that domain: the domain is
/// named by its ARN, the one identifier of it that is unique across accounts and Regions.
pub const DOMAIN_ARN_CONTEXT_KEY: &str = "domainARN";

/// What the grant permits, which is what the service does with the key for an
/// encrypted-at-rest domain: it asks the key for the data key the domain's storage is
/// encrypted under (`GenerateDataKey`), asks the key to decrypt that data key whenever the
/// storage is read (`Decrypt`), and reads the key's own metadata to tell whether it can
/// still be used at all (`DescribeKey`).
///
/// Nothing wider: a grant is a permission on a customer's key, so it names the operations
/// the storage needs and leaves every other one to the key's policy. Each is a value of the
/// KMS model's `GrantOperation` enum
/// (`smithy_query(query="shape",shape="GrantOperation")`), and an operation the key cannot
/// perform is left out ([`permitted_operations`]) — "the operation must be supported on the
/// KMS key"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`).
pub const GRANT_OPERATIONS: &[&str] = &["Decrypt", "DescribeKey", "GenerateDataKey"];

/// The principal the grant is for: this service, in `region`.
///
/// A service principal is `<service>.amazonaws.com`, and the REGIONAL form
/// `<service>.<region>.amazonaws.com` names the service in one Region — which is the one the
/// grant belongs to, since a domain and the key its data is encrypted under are both
/// regional resources answering at that Region's endpoint. The service is named by its own
/// namespace ([`super::ARN_NAMESPACE`], the `es` its ARNs, its endpoint and its signing name
/// all carry), so the two cannot drift.
pub fn service_principal(region: &str) -> String {
    format!("{}.{region}.amazonaws.com", super::ARN_NAMESPACE)
}

/// Bring the grants KMS holds for the domain `domain_arn` into line with the encryption
/// `options` leave it with: one grant on the key its data is encrypted under, and none
/// anywhere else.
///
/// `region` is the Region the domain lives in, which is the Region its key answers at and
/// so the Region the grant belongs to. `options` is the container the domain HOLDS after
/// the call — `None`, or one that enables no encryption, for a domain whose data is not
/// encrypted at rest and for one that no longer exists.
///
/// Idempotent: a call that leaves the domain encrypted under the key it was already
/// encrypted under keeps the grant it already holds rather than taking a second one, so no
/// sequence of updates grows the key's grants.
pub fn settle_domain_grant<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    domain_arn: &str,
    options: Option<&EncryptionAtRestOptions>,
) {
    let Some(state) = kms_store::service_state(engine) else {
        return;
    };
    let mut keys = state.lock();

    // The key the domain's data is encrypted under, as KMS answers for it.
    let encrypting = match encrypting_key(&mut keys, region, options) {
        Encrypting::Under(key) => Some(key),
        Encrypting::NotAtRest => None,
        // KMS answers for no such key, which is a configuration the create and update
        // paths refuse before they reach here ([`super::encryption_at_rest`]). Nothing is
        // known about the domain's key, so nothing is changed: a permission the service
        // was not asked about is not taken away.
        Encrypting::Unresolved => return,
    };

    // What this domain already holds. A grant is recognised by what it SAYS — this
    // service's principal, constrained to this domain — rather than by an id this service
    // would have to remember, so one domain holds one grant whichever call took it, and a
    // grant a client revoked stays revoked.
    let held: Vec<(String, String)> = keys
        .grants_in(region)
        .into_iter()
        .filter(|grant| grants_this_domain(grant, region, domain_arn))
        .map(|grant| (grant.grant_id.clone(), grant.key_id.clone()))
        .collect();
    let mut on_the_live_key = false;
    for (grant_id, key_id) in held {
        if encrypting
            .as_ref()
            .is_some_and(|key| key.key_id == key_id)
        {
            on_the_live_key = true;
        } else {
            // The domain's data is no longer encrypted under this key — the encryption was
            // switched off, moved to another key, or the domain is gone — so the
            // permission it gave the service goes back.
            keys.remove_grant(region, &grant_id);
        }
    }

    let Some(key) = encrypting else {
        return;
    };
    if on_the_live_key {
        return;
    }
    // The KMS store is bounded, by the same bounds its own `CreateGrant` keeps: past them
    // no grant is written, exactly as that operation writes none.
    if !keys.has_room_for_another_grant()
        || keys.grants_for_key(region, &key.key_id).len() >= kms_store::MAX_GRANTS_PER_KEY
    {
        return;
    }
    // A grant id is drawn from the OS CSPRNG, as every KMS grant id is; a platform that
    // refuses entropy leaves no grant rather than a predictable id.
    let Some(grant_id) = kms_store::mint_grant_id() else {
        return;
    };
    let principal = service_principal(region);
    keys.insert_grant(kms_store::Grant {
        grant_id,
        key_id: key.key_id,
        key_arn: key.key_arn,
        region: region.to_string(),
        issuing_account: kms_store::issuing_account_arn(),
        // "If a name was provided in the CreateGrant request, that name is returned.
        // Otherwise this value is null"
        // (`smithy_query(query="doc",shape="GrantListEntry",member="Name")`): no request
        // made this grant, so it carries no name.
        name: None,
        creation_date: SystemTime::now(),
        grantee_principal: Some(principal.clone()),
        grantee_service_principal: None,
        retiring_principal: Some(principal),
        retiring_service_principal: None,
        operations: key.operations,
        constraints: Some(GrantConstraints {
            encryption_context_subset: Some(HashMap::from([(
                DOMAIN_ARN_CONTEXT_KEY.to_string(),
                domain_arn.to_string(),
            )])),
            ..GrantConstraints::default()
        }),
    });
}

/// The key the domain's data is encrypted under, as KMS answers for it.
struct EncryptingKey {
    /// The key id of the KMS key, which is what a grant records: a grant names the key it
    /// RESOLVED to, so repointing an alias moves no grant.
    key_id: String,
    /// That key's ARN, which is what a grant reader reports the grant's key as.
    key_arn: String,
    /// The operations this key can carry of the ones the storage needs
    /// ([`permitted_operations`]).
    operations: Vec<String>,
}

/// What KMS answers about the key `options` leave the domain's data encrypted under.
enum Encrypting {
    /// That key: the domain's data is encrypted under it.
    Under(EncryptingKey),
    /// The domain's data is not encrypted at rest at all, which is also what a domain that
    /// no longer exists leaves behind.
    NotAtRest,
    /// The container names a key KMS answers for nothing about.
    Unresolved,
}

/// The key `options` leave the domain's data encrypted under, resolved in KMS's own store.
fn encrypting_key(
    keys: &mut KmsState,
    region: &str,
    options: Option<&EncryptionAtRestOptions>,
) -> Encrypting {
    let Some(options) = options else {
        return Encrypting::NotAtRest;
    };
    // "Specifies whether the domain should encrypt data at rest": with no encryption there
    // is no data key, no key in use and nothing to permit.
    if !options.enabled.unwrap_or(false) {
        return Encrypting::NotAtRest;
    }
    // A configuration that enables encryption without naming a key of its own encrypts
    // under the Amazon Web Services managed key of THIS service
    // ([`super::encryption_at_rest::AWS_MANAGED_KEY_ALIAS`]) — the key a domain uses when
    // the caller names none — so that is the key the grant goes on.
    let requested = options
        .kms_key_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or(super::encryption_at_rest::AWS_MANAGED_KEY_ALIAS);
    // A predefined Amazon Web Services alias carries no key until one is asked for: "if you
    // specify a predefined Amazon Web Services alias (an Amazon Web Services alias with no
    // key ID), KMS associates the alias with an Amazon Web Services managed key"
    // (`smithy_query(query="doc",shape="DescribeKeyRequest",member="KeyId")` in the KMS
    // model), and the service asking for the key to encrypt a domain with is such a call.
    if let Some(alias) = kms_store::alias_named_by(region, requested) {
        if kms_store::is_predefined_aws_alias(&alias) {
            return match keys.associate_predefined_aws_alias(region, &alias) {
                Some(key) => Encrypting::Under(described(key)),
                None => Encrypting::Unresolved,
            };
        }
    }
    match keys.resolve(region, requested) {
        kms_store::KeyRef::Found(key) => Encrypting::Under(described(key)),
        kms_store::KeyRef::NotFound
        | kms_store::KeyRef::MalformedArn
        | kms_store::KeyRef::UnauthorizedAccount => Encrypting::Unresolved,
    }
}

/// What this service records of the key KMS answered for.
fn described(key: &kms_store::Cmk) -> EncryptingKey {
    EncryptingKey {
        key_id: key.key_id.clone(),
        key_arn: key.arn.clone(),
        operations: permitted_operations(key),
    }
}

/// The [`GRANT_OPERATIONS`] the key `key` supports, read with KMS's own rule for what a
/// grant on a key may permit
/// (`service_com_amazonaws_kms_trentservice::state::grant_operation_refusal`) rather than a
/// second account of key material here — "you cannot create a grant … for an asymmetric KMS
/// key that allows the GenerateDataKey operation"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`), so an
/// operation the key cannot perform is one no grant on it carries.
fn permitted_operations(key: &kms_store::Cmk) -> Vec<String> {
    GRANT_OPERATIONS
        .iter()
        .filter(|operation| kms_store::grant_operation_refusal(key, operation).is_none())
        .map(|operation| (*operation).to_string())
        .collect()
}

/// Whether `grant` is the grant the service holds for the domain `domain_arn`: one for this
/// service's principal in `region`, constrained to that domain's data
/// ([`DOMAIN_ARN_CONTEXT_KEY`]).
fn grants_this_domain(grant: &kms_store::Grant, region: &str, domain_arn: &str) -> bool {
    grant.grantee_principal.as_deref() == Some(service_principal(region).as_str())
        && grant
            .constraints
            .as_ref()
            .and_then(|constraints| constraints.encryption_context_subset.as_ref())
            .and_then(|context| context.get(DOMAIN_ARN_CONTEXT_KEY))
            .map(String::as_str)
            == Some(domain_arn)
}
