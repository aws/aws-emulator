// SPDX-License-Identifier: CC0-1.0

//! One revoke, from the request that asks for it to the records it trims.
//!
//! `RevokePermissions` revokes permissions from a principal and `BatchRevokePermissions` is
//! the "Batch operation to revoke permissions from the principal" — the same revoke, asked
//! for one entry at a time — so both read a revoke HERE, exactly as the grant pair share
//! [`super::read_grant_request`] and [`super::plan_grant`]. A batch entry is therefore
//! accepted for exactly what the single-entry call accepts and refused for exactly what it
//! refuses.
//!
//! The order the refusals come in, fixed here so the two paths cannot drift:
//!
//! 1. every value named is a `Permission` the model carries;
//! 2. the grantee identifies a Lake Formation principal ([`malformed_principal`]);
//! 3. at least one permission is named;
//! 4. the `Resource` names exactly one resource, at a scope its permissions apply at;
//! 5. the grantee holds, at every scope the request reaches, every permission it names —
//!    else nothing is revoked, worded by whether this service models the grantee on the
//!    resource at all ([`NOTHING_TO_REVOKE`] against [`permissions_not_held`]).

use super::*;

/// What a revoke request that names no permission at all is refused with, the counterpart
/// of [`NO_PERMISSION_TO_GRANT`].
pub const NO_PERMISSION_TO_REVOKE: &str =
    "Permissions must name at least one permission to revoke";

/// What one revoke request ASKS FOR: the grantee, the two permission sets as sets, and the
/// condition the grant it addresses is scoped by.
#[derive(Debug, Clone)]
pub struct RevokeRequest {
    /// The grantee — read through [`malformed_principal`].
    pub principal: String,
    /// `Permissions`, de-duplicated in first-named order.
    pub permissions: Vec<String>,
    /// `PermissionsWithGrantOption`, de-duplicated in first-named order.
    pub permissions_with_grant_option: Vec<String>,
    /// `Condition$Expression` when the request carries a non-empty one: a grant under a
    /// condition is a different grant.
    pub condition: Option<String>,
}

impl RevokeRequest {
    /// Every permission this request asks to take away, in the order it named them:
    /// `Permissions` first, then each `PermissionsWithGrantOption` value it did not already
    /// name — one value names one permission however often it is written.
    ///
    /// What the refusal that reports the difference ([`permissions_not_held`]) lists.
    pub fn named_permissions(&self) -> Vec<String> {
        dedup_values(
            self.permissions
                .iter()
                .chain(self.permissions_with_grant_option.iter()),
        )
    }
}

/// What one revoke will take away, resolved before this service's store is locked.
#[derive(Debug, Clone)]
pub struct RevokePlan {
    /// The resource the request named, normalised ([`normalize_resource`]).
    pub resource: Resource,
    /// The key of the record the request itself addresses — the one a database's
    /// creation-time default grant is remembered under.
    pub key: String,
    /// The (key, permissions, permissions-with-grant-option) triple per SCOPE the request
    /// reaches, as [`scoped_grant_records`] splits it.
    pub scoped: Vec<(String, Vec<String>, Vec<String>)>,
    /// The grants the addressed TABLE was created carrying that belong to this grantee.
    pub table_grants: Vec<(String, PermissionGrant)>,
}

/// What a revoke DID, for the audit record: per-record counts and whether a grant that is
/// not a record was taken away.
#[derive(Debug, Clone, Copy, Default)]
pub struct RevokeOutcome {
    /// Records emptied and dropped.
    pub removed: usize,
    /// Records trimmed but still holding something.
    pub reduced: usize,
    /// Whether a database's creation-time default grant was taken away.
    pub revoked_default: bool,
    /// Whether a grant the table was created carrying was taken away.
    pub revoked_implicit: bool,
}

/// Steps 1 to 3 of the order above: the members of a revoke request that do not depend on
/// the resource it names.
///
/// Every member is optional here because a `BatchPermissionsRequestEntry` models them all
/// optional; a request that names none of them is refused for the first it is missing.
pub fn read_revoke_request(
    principal: Option<&DataLakePrincipal>,
    permissions: &[String],
    permissions_with_grant_option: &[String],
    condition: Option<&Condition>,
) -> Result<RevokeRequest, String> {
    // The same reading of the permission lists the grant path makes, and for the same
    // reason: a value the `Permission` enum does not carry names no permission, so the
    // request is refused before anything is read or taken away.
    let rejected =
        unmodelled_permissions(permissions.iter().chain(permissions_with_grant_option.iter()));
    if !rejected.is_empty() {
        return Err(invalid_permissions(&rejected));
    }

    let identifier = principal
        .and_then(|principal| principal.data_lake_principal_identifier.as_deref())
        .unwrap_or_default();
    if let Some(refusal) = malformed_principal(identifier) {
        return Err(refusal);
    }

    let permissions = dedup_values(permissions.iter());
    let permissions_with_grant_option = dedup_values(permissions_with_grant_option.iter());
    if permissions.is_empty() && permissions_with_grant_option.is_empty() {
        return Err(NO_PERMISSION_TO_REVOKE.to_string());
    }

    Ok(RevokeRequest {
        principal: identifier.to_string(),
        permissions,
        permissions_with_grant_option,
        condition: condition
            .and_then(|condition| condition.expression.clone())
            .filter(|expression| !expression.is_empty()),
    })
}

/// Step 4 of the order above, and everything the revoke will reach.
///
/// A revoke takes away what the matching GRANT wrote, so it is split the same way: a
/// table's column-scoped permissions were recorded against that table's `TableWithColumns`
/// key, and naming the TABLE has to reach them ([`scoped_grant_records`]).
///
/// Call this BEFORE this service's own store is locked: [`created_table_grants`] takes
/// Glue's lock itself.
pub fn plan_revoke<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    request: &RevokeRequest,
    resource: Option<&Resource>,
) -> Result<RevokePlan, String> {
    let Some(resource) = resource else {
        return Err(RESOURCE_NAMES_NO_TYPE.to_string());
    };
    validate_resource(resource)?;

    // The same scope rule the grant path applies, so the two paths agree on what a
    // column-scoped request may say.
    if let Some(violation) = column_scope_violation(
        resource,
        &request.permissions,
        &request.permissions_with_grant_option,
    ) {
        return Err(violation);
    }

    // Lake Formation revokes permissions ON Data Catalog resources, so one the catalog does
    // not hold names nothing to revoke FROM: the entity is resolved exactly as the grant
    // path resolves it ([`missing_grant_entity`]), and before the permissions are weighed,
    // so a revoke on an absent entity is refused for the entity rather than for the
    // permissions nothing could hold there.
    if let Some(missing) = missing_grant_entity(engine, resource, catalog_id) {
        return Err(missing);
    }

    let normalized = normalize_resource(resource, catalog_id);
    let key = grant_key(
        catalog_id,
        &request.principal,
        &normalized,
        request.condition.as_deref(),
    );

    // The grants the addressed TABLE was created carrying, which are not records either.
    // Only this principal's are of interest here, and such a grant carries no condition, so
    // a condition-scoped request addresses a different grant.
    let table_grants: Vec<(String, PermissionGrant)> = match &request.condition {
        Some(_) => Vec::new(),
        None => created_table_grants(engine, catalog_id, Some(&normalized))
            .into_iter()
            .filter(|(_, grant)| grant.principal == request.principal)
            .collect(),
    };

    let scoped: Vec<(String, Vec<String>, Vec<String>)> = scoped_grant_records(
        resource,
        &request.permissions,
        &request.permissions_with_grant_option,
    )
    .into_iter()
    .map(|(scoped_resource, permissions, permissions_with_grant_option)| {
        let scoped_resource = normalize_resource(&scoped_resource, catalog_id);
        (
            grant_key(
                catalog_id,
                &request.principal,
                &scoped_resource,
                request.condition.as_deref(),
            ),
            permissions,
            permissions_with_grant_option,
        )
    })
    .collect();

    Ok(RevokePlan {
        resource: normalized,
        key,
        scoped,
        table_grants,
    })
}

/// Step 5 of the order above, and the revoke itself: takes the named permissions out of
/// every record the plan reaches, or says why nothing could be taken away.
///
/// Everything the grantee does not hold, whether because the grant does not exist at all or
/// because it never carried these permissions, takes nothing away and is refused — but the
/// two refusal CLASSES are not worded alike: a grantee this service models on the resource
/// is told WHICH of the named permissions it does not have ([`permissions_not_held`]), while
/// a grantee that holds nothing there at all draws the sentence about holding nothing
/// ([`NOTHING_TO_REVOKE`]). The grantee the service always models is the
/// [`IAM_ALLOWED_PRINCIPALS`] group, which stands for every principal IAM itself allows and
/// is therefore never absent from a resource's permission model, however little it holds
/// there.
pub fn apply_revoke(
    store: &mut ComAmazonawsLakeformationAwslakeformationState,
    catalog_id: &str,
    request: &RevokeRequest,
    plan: &RevokePlan,
    provenance: &GrantProvenance,
) -> Result<RevokeOutcome, String> {
    // The creation-time default, unless a revoke already took it away. It is not a record,
    // so it is held ON TOP of whatever `GrantPermissions` wrote. What the default IS comes
    // from the addressed catalog's own data lake settings, as `PutDataLakeSettings` last
    // wrote them.
    let settings = store.data_lake_settings_for(catalog_id);
    let default_grant = default_database_grant(
        &settings,
        &plan.resource,
        &request.principal,
        request.condition.as_deref(),
    )
    .filter(|_| !store.revoked_default_grants.contains(&plan.key));

    let mut missing: Vec<String> = Vec::new();
    // Whether the grantee holds ANYTHING at a scope this revoke reaches, which is what
    // decides between the two nothing-was-revoked sentences below.
    let mut holds_something = false;
    for (scoped_key, permissions, permissions_with_grant_option) in &plan.scoped {
        let held = store.permission_grants.get(scoped_key);
        let mut held_permissions: Vec<String> = held
            .map(|grant| grant.permissions.clone())
            .unwrap_or_default();
        // The default grant belongs to the resource the request named, which is a database
        // — a resource that never splits, so it is this one record.
        if scoped_key == &plan.key {
            if let Some(default) = &default_grant {
                held_permissions.extend(default.iter().cloned());
            }
        }
        // The default grant carries no grant option, so only a record can hold one.
        let mut held_with_grant_option: Vec<String> = held
            .map(|grant| grant.permissions_with_grant_option.clone())
            .unwrap_or_default();
        // A table holds what its creation granted on top of whatever was granted
        // explicitly, at this scope, unless a revoke already took it away.
        if !store.revoked_default_grants.contains(scoped_key) {
            if let Some(implicit) = implicit_grant_for(&plan.table_grants, scoped_key) {
                held_permissions.extend(implicit.permissions.iter().cloned());
                held_with_grant_option
                    .extend(implicit.permissions_with_grant_option.iter().cloned());
            }
        }
        holds_something |= !held_permissions.is_empty() || !held_with_grant_option.is_empty();
        missing.extend(missing_permissions(&held_permissions, permissions));
        missing.extend(missing_permissions(
            &held_with_grant_option,
            permissions_with_grant_option,
        ));
    }
    if !missing.is_empty() {
        // A grantee this service models on the resource — the group it always models, or a
        // principal holding something at a scope the revoke reaches — is told the
        // difference, with the permissions the request named; one that holds nothing there
        // draws the wholly fixed sentence about holding nothing.
        let modelled_on_resource = request.principal == IAM_ALLOWED_PRINCIPALS || holds_something;
        return Err(if modelled_on_resource {
            permissions_not_held(&request.named_permissions())
        } else {
            NOTHING_TO_REVOKE.to_string()
        });
    }

    // Taking a default grant away leaves no record to trim, so what is left behind is the
    // memory that it is gone — a record of its own, under its own ceiling.
    let revoked_default = default_grant
        .as_ref()
        .is_some_and(|default| default.iter().any(|granted| request.permissions.contains(granted)));
    if revoked_default {
        if store.revoked_default_grants.len() >= MAX_REVOKED_DEFAULT_GRANTS
            && !store.revoked_default_grants.contains(&plan.key)
        {
            return Err(format!(
                "The revoke of the default grant could not be recorded: {} revoked \
                 default grants are already held, the maximum is {MAX_REVOKED_DEFAULT_GRANTS}",
                store.revoked_default_grants.len()
            ));
        }
        store.revoked_default_grants.insert(plan.key.clone());
    }

    // A grant a table was created carrying is taken away the same way — remembered as gone,
    // keyed exactly as the listing renders it — and whatever it still holds becomes a
    // record, so a revoke of one permission leaves the others in place.
    let revoked_implicit =
        revoke_implicit_grants(store, &plan.table_grants, &plan.scoped, provenance)?;

    // Counted per record, symmetrically with the grant path's inserted/updated: a revoke can
    // trim one scope and empty another, and a revoke of a default grant trims no record.
    let mut outcome = RevokeOutcome {
        revoked_default,
        revoked_implicit,
        ..RevokeOutcome::default()
    };
    for (scoped_key, permissions, permissions_with_grant_option) in &plan.scoped {
        if let Some(grant) = store.permission_grants.get_mut(scoped_key) {
            remove_permissions(&mut grant.permissions, permissions);
            remove_permissions(
                &mut grant.permissions_with_grant_option,
                permissions_with_grant_option,
            );
            // A record with nothing left in either set is no longer a grant: drop it, so a
            // later `ListPermissions` reports no entry rather than an entry with no
            // permissions.
            if grant.permissions.is_empty() && grant.permissions_with_grant_option.is_empty() {
                store.permission_grants.remove(scoped_key);
                outcome.removed += 1;
            } else {
                // What the record still holds was last written by THIS revoke, so that is
                // the provenance a later read of it reports.
                grant.written_by(provenance);
                outcome.reduced += 1;
            }
        }
    }
    Ok(outcome)
}
