// SPDX-License-Identifier: CC0-1.0

//! Taking away a grant a Data Catalog resource was CREATED carrying.
//!
//! The sibling module `creator_table_grants` renders those grants per request
//! ([`super::created_table_grants`]); this one is
//! the write side. Such a grant is not a record — nothing granted it, so there is nothing
//! in
//! [`super::ComAmazonawsLakeformationAwslakeformationState::permission_grants`] for
//! `RevokePermissions` to trim — so what a revoke leaves behind is the memory that the
//! rendered grant is gone, together with a record of whatever it still holds.

use super::*;

/// The implicit grant among `implicit` that is held under `key`, if any.
pub fn implicit_grant_for<'a>(
    implicit: &'a [(String, PermissionGrant)],
    key: &str,
) -> Option<&'a PermissionGrant> {
    implicit
        .iter()
        .find(|(held_key, _)| held_key == key)
        .map(|(_, grant)| grant)
}

/// Takes the permissions `revoked` names out of the implicit grants `implicit` renders,
/// answering whether any of them was taken away — or why the revoke could not be recorded.
///
/// What a revoke leaves behind is the memory that the rendered grant is gone
/// (`revoked_default_grants`, keyed exactly as the listing renders it) together with a
/// record of whatever the grant still holds — a table's creator that loses one permission
/// keeps the others, and only a record can say so once the rendered grant is suppressed.
///
/// `revoked` is the (key, permissions, permissions-with-grant-option) triple per SCOPE the
/// request reaches, as [`scoped_grant_records`] splits it. The caller has already refused a
/// revoke naming anything the grantee does not hold.
///
/// Every scope is weighed against both ceilings BEFORE the first write, so a refusal leaves
/// no half of a multi-scope revoke behind.
pub fn revoke_implicit_grants(
    store: &mut ComAmazonawsLakeformationAwslakeformationState,
    implicit: &[(String, PermissionGrant)],
    revoked: &[(String, Vec<String>, Vec<String>)],
    provenance: &GrantProvenance,
) -> Result<bool, String> {
    // What this revoke takes away, per scope: the key to remember as revoked, and what the
    // grant still holds there (`None` when nothing is left of it).
    let mut taking: Vec<(&String, Option<PermissionGrant>)> = Vec::new();
    for (key, permissions, permissions_with_grant_option) in revoked {
        if store.revoked_default_grants.contains(key) {
            continue;
        }
        let Some(grant) = implicit_grant_for(implicit, key) else {
            continue;
        };
        let touches = grant
            .permissions
            .iter()
            .any(|held| permissions.contains(held))
            || grant
                .permissions_with_grant_option
                .iter()
                .any(|held| permissions_with_grant_option.contains(held));
        if !touches {
            continue;
        }

        let mut remainder = grant.clone();
        remove_permissions(&mut remainder.permissions, permissions);
        remove_permissions(
            &mut remainder.permissions_with_grant_option,
            permissions_with_grant_option,
        );
        let keeps_remainder = !(remainder.permissions.is_empty()
            && remainder.permissions_with_grant_option.is_empty());
        taking.push((key, keeps_remainder.then_some(remainder)));
    }
    if taking.is_empty() {
        return Ok(false);
    }

    // The memory of each revoke is a record of its own, under its own ceiling, since its
    // key carries request-supplied names. Weighed for the whole revoke first.
    let remembered = taking.len();
    if store.revoked_default_grants.len() + remembered > MAX_REVOKED_DEFAULT_GRANTS {
        return Err(format!(
            "The revoke of the default grant could not be recorded: {} revoked default \
             grants are already held and this revoke needs {remembered} more, the maximum \
             is {MAX_REVOKED_DEFAULT_GRANTS}",
            store.revoked_default_grants.len()
        ));
    }
    // What a grant still holds has to become a record now that the rendered grant is
    // suppressed, so the grant store's own ceiling is weighed for the whole revoke too.
    let inserts = taking
        .iter()
        .filter(|(key, remainder)| {
            remainder.is_some() && !store.permission_grants.contains_key(*key)
        })
        .count();
    if store.permission_grants.len() + inserts > MAX_PERMISSION_GRANTS {
        return Err(format!(
            "{} permission grants are already held and this revoke needs {inserts} more to \
             record what is left of the grant, the maximum is {MAX_PERMISSION_GRANTS}",
            store.permission_grants.len()
        ));
    }

    for (key, remainder) in taking {
        store.revoked_default_grants.insert(key.clone());
        let Some(remainder) = remainder else {
            continue;
        };
        match store.permission_grants.get_mut(key) {
            Some(held) => {
                merge_permissions(&mut held.permissions, &remainder.permissions);
                merge_permissions(
                    &mut held.permissions_with_grant_option,
                    &remainder.permissions_with_grant_option,
                );
                held.written_by(provenance);
            }
            None => {
                let mut remainder = remainder;
                // The remainder is a record THIS revoke wrote, so it carries this call's
                // provenance rather than the one the rendered grant was created with.
                remainder.written_by(provenance);
                store.permission_grants.insert(key.clone(), remainder);
            }
        }
    }
    Ok(true)
}
