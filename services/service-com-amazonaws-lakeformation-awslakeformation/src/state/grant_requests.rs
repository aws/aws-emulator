// SPDX-License-Identifier: CC0-1.0

//! One grant, from the request that asks for it to the records it writes.
//!
//! `GrantPermissions` grants permissions to a principal and `BatchGrantPermissions` is the
//! "Batch operation to grant permissions to the principal" — the same grant, asked for one
//! entry at a time — so both read a grant HERE. A batch entry is therefore accepted for
//! exactly what the single-grant call accepts and refused for exactly what it refuses, and
//! what a non-failed entry writes is the record `ListPermissions` reads back.
//!
//! The order the refusals come in is fixed here, so the two paths cannot drift on which of
//! several bad members a request is refused for:
//!
//! 1. every value named is a `Permission` the model carries;
//! 2. the grantee identifies a Lake Formation principal ([`malformed_principal`]);
//! 3. at least one permission is named, and the grant option is asked for only on a
//!    permission the grant grants ([`grant_option_outside_permissions`]);
//! 4. (the caller's own step, because it is asynchronous) the grantee resolves in IAM;
//! 5. the `Resource` names exactly one resource, whose kind holds every permission named
//!    ([`inapplicable_permissions`]);
//! 6. the Data Catalog holds the entity it names.

use super::*;

/// The batch entry shape a failure echoes back; reached by name because `state.rs`'s own
/// imports do not carry it.
use crate::ops::types::BatchPermissionsRequestEntry;

/// What one grant request ASKS FOR, once the members that do not depend on the resource
/// have been read: the grantee, the two permission sets as sets, and the condition the
/// grant is scoped by.
#[derive(Debug, Clone)]
pub struct GrantRequest {
    /// The grantee — `DataLakePrincipal$DataLakePrincipalIdentifier`, read through
    /// [`malformed_principal`].
    pub principal: String,
    /// `Permissions`, de-duplicated in first-named order.
    pub permissions: Vec<String>,
    /// `PermissionsWithGrantOption`, de-duplicated in first-named order.
    pub permissions_with_grant_option: Vec<String>,
    /// `Condition$Expression` when the request carries a non-empty one: part of the
    /// identity of every record this grant writes.
    pub condition: Option<String>,
}

/// One record a grant writes: the key that identifies it, the resource at the scope the
/// record is held at, and the permissions held there.
#[derive(Debug, Clone)]
pub struct GrantRecord {
    /// [`grant_key`] of the (catalog, principal, resource, condition) tuple.
    pub key: String,
    /// The resource, normalised ([`normalize_resource`]) so the same resource named twice
    /// is one record.
    pub resource: Resource,
    /// What this scope holds.
    pub permissions: Vec<String>,
    /// What this scope holds WITH the grant option.
    pub permissions_with_grant_option: Vec<String>,
}

/// What a grant request that names no permission at all is refused with.
///
/// `PermissionList` carries no length constraint, so the boundary validator admits an empty
/// list. A grant of no permission would record a grantee with nothing granted, which no
/// read could tell from no grant.
pub const NO_PERMISSION_TO_GRANT: &str = "Permissions must name at least one permission to grant";

/// The most entries one batch of permission requests may carry: "A list of up to 20
/// entries for resource permissions to be granted by batch operation to the principal"
/// (`smithy_query(query="shape",shape="BatchGrantPermissionsRequest")`, member `Entries`).
pub const MAX_BATCH_PERMISSIONS_ENTRIES: usize = 20;

/// What a batch naming more entries than [`MAX_BATCH_PERMISSIONS_ENTRIES`] is refused with.
///
/// The limit is on the request as a whole rather than on one entry, so this is the batch's
/// own modelled `InvalidInputException` and not a per-entry failure. Emulator-worded, like
/// the store ceilings: the model states the limit but not the sentence.
pub fn too_many_batch_entries(named: usize) -> String {
    format!(
        "Entries names {named} entries, the maximum is {MAX_BATCH_PERMISSIONS_ENTRIES}"
    )
}

/// The `ErrorDetail$ErrorCode` a failed batch entry reports: the error class the
/// single-entry call answers the same refusal with.
///
/// Every refusal a batch entry can draw is one the single call answers with its modelled
/// `InvalidInputException` — a value that is not a permission, a grantee that identifies
/// nobody, a resource the Data Catalog does not hold — so that is the class the entry's
/// failure names.
pub const BATCH_ENTRY_ERROR_CODE: &str = "InvalidInputException";

/// The entry a batch failure reports back, as the service RESOLVED it rather than as the
/// request wrote it: the catalog ids the request left to their default are filled in
/// ([`resolve_resource_catalogs`]) and both permission lists are present, empty where the
/// request named none.
///
/// A failure reports `RequestEntry` ("An identifier for an entry of the batch request") so
/// the caller can match the failure to what it asked for; what it matches is the request as
/// the service read it, which is also what the entry would have been recorded as. Nothing
/// is folded or reordered: the names, the permissions and the condition are the request's
/// own values.
pub fn echoed_batch_entry(
    entry: &BatchPermissionsRequestEntry,
    catalog_id: &str,
) -> BatchPermissionsRequestEntry {
    BatchPermissionsRequestEntry {
        id: entry.id.clone(),
        principal: entry.principal.clone(),
        resource: entry
            .resource
            .as_ref()
            .map(|resource| resolve_resource_catalogs(resource, catalog_id)),
        permissions: Some(entry.permissions.clone().unwrap_or_default()),
        condition: entry.condition.clone(),
        permissions_with_grant_option: Some(
            entry.permissions_with_grant_option.clone().unwrap_or_default(),
        ),
    }
}

/// Steps 1 to 3 of the order above: the members of a grant request that do not depend on
/// the resource it names.
///
/// Every member is optional here because a `BatchPermissionsRequestEntry` models them all
/// optional; a request that names none of them is refused for the first it is missing.
pub fn read_grant_request(
    principal: Option<&DataLakePrincipal>,
    permissions: &[String],
    permissions_with_grant_option: &[String],
    condition: Option<&Condition>,
) -> Result<GrantRequest, String> {
    // A value the `Permission` enum does not carry names no permission at all, and the
    // service refuses the whole request for it, listing the values it rejected.
    let rejected =
        unmodelled_permissions(permissions.iter().chain(permissions_with_grant_option.iter()));
    if !rejected.is_empty() {
        return Err(invalid_permissions(&rejected));
    }

    // A grant is made TO a principal, so a value that identifies none — including no value
    // at all — is refused before the grant is resolved any further.
    let identifier = principal
        .and_then(|principal| principal.data_lake_principal_identifier.as_deref())
        .unwrap_or_default();
    if let Some(refusal) = malformed_principal(identifier) {
        return Err(refusal);
    }

    let permissions = dedup_values(permissions.iter());
    let permissions_with_grant_option = dedup_values(permissions_with_grant_option.iter());
    if permissions.is_empty() && permissions_with_grant_option.is_empty() {
        return Err(NO_PERMISSION_TO_GRANT.to_string());
    }

    // The grant option may only be handed out for a permission the grant itself grants:
    // `PermissionsWithGrantOption` "may only be a subset of the permissions granted in the
    // Privileges", so a grant that passes on what it does not grant is not a modification
    // Lake Formation can make ([`grant_option_outside_permissions`]).
    if let Some(refusal) =
        grant_option_outside_permissions(&permissions, &permissions_with_grant_option)
    {
        return Err(refusal);
    }

    Ok(GrantRequest {
        principal: identifier.to_string(),
        permissions,
        permissions_with_grant_option,
        condition: condition
            .and_then(|condition| condition.expression.clone())
            .filter(|expression| !expression.is_empty()),
    })
}

/// Steps 5 and 6 of the order above, and the records the grant writes: one per SCOPE the
/// requested permissions apply at ([`scoped_grant_records`]).
///
/// Call this BEFORE this service's own store is locked: [`missing_grant_entity`] reads
/// Glue's store and takes its lock, and holding both at once is what would deadlock
/// against a handler that takes them in the other order.
pub fn plan_grant<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog_id: &str,
    request: &GrantRequest,
    resource: Option<&Resource>,
) -> Result<Vec<GrantRecord>, String> {
    // `Resource` is modelled required on `GrantPermissions` and optional on a batch entry,
    // so an entry naming none is refused exactly as a `Resource` naming no member is.
    let Some(resource) = resource else {
        return Err(RESOURCE_NAMES_NO_TYPE.to_string());
    };
    validate_resource(resource)?;

    // Each member of `Resource` names a different KIND of resource, and only the
    // privileges that kind has mean anything on it — `TableWithColumnsResource` "is only
    // used when granting a SELECT permission" is the one the model states outright — so a
    // grant naming a permission the named kind cannot hold is not a modification Lake
    // Formation can make and is refused rather than recorded at a scope no read could
    // honour ([`inapplicable_permissions`]).
    if let Some(violation) = inapplicable_permissions(
        resource,
        &request.permissions,
        &request.permissions_with_grant_option,
    ) {
        return Err(violation);
    }

    // Lake Formation grants permissions ON a resource, so one the emulator does not hold
    // names nothing and is refused with nothing persisted.
    if let Some(missing) = missing_grant_entity(engine, resource, catalog_id) {
        return Err(missing);
    }

    Ok(scoped_grant_records(
        resource,
        &request.permissions,
        &request.permissions_with_grant_option,
    )
    .into_iter()
    .map(|(resource, permissions, permissions_with_grant_option)| {
        let resource = normalize_resource(&resource, catalog_id);
        GrantRecord {
            key: grant_key(
                catalog_id,
                &request.principal,
                &resource,
                request.condition.as_deref(),
            ),
            resource,
            permissions,
            permissions_with_grant_option,
        }
    })
    .collect())
}

/// Writes `records` into the grant store, answering how many records were inserted and how
/// many an existing record absorbed — or why the grant could not be recorded.
///
/// Granting again on the same resource to the same principal UNIONS the permissions into
/// the record it already has, so a repeated grant is a no-op on state rather than a second
/// entry — and `provenance` is written onto every record this call touched, inserted or
/// absorbed, because that write is the one a later read reports as the record's last.
///
/// This is the single site that grows the grant store, so this is where
/// [`MAX_PERMISSION_GRANTS`] is enforced. The whole grant is weighed BEFORE the first
/// write, so a refusal leaves no half of a split grant behind.
pub fn record_grants(
    store: &mut ComAmazonawsLakeformationAwslakeformationState,
    catalog_id: &str,
    request: &GrantRequest,
    records: &[GrantRecord],
    provenance: &GrantProvenance,
) -> Result<(usize, usize), String> {
    let held = store.permission_grants.len();
    let inserts = records
        .iter()
        .filter(|record| !store.permission_grants.contains_key(&record.key))
        .count();
    if held + inserts > MAX_PERMISSION_GRANTS {
        return Err(format!(
            "{held} permission grants are already held and this grant needs {inserts} \
             more, the maximum is {MAX_PERMISSION_GRANTS}"
        ));
    }

    let mut inserted = 0_usize;
    let mut updated = 0_usize;
    for record in records {
        match store.permission_grants.get_mut(&record.key) {
            // The same principal, resource and condition: one record, whose permission
            // sets grow by what this call adds.
            Some(grant) => {
                merge_permissions(&mut grant.permissions, &record.permissions);
                merge_permissions(
                    &mut grant.permissions_with_grant_option,
                    &record.permissions_with_grant_option,
                );
                grant.written_by(provenance);
                updated += 1;
            }
            None => {
                let mut grant = PermissionGrant {
                    catalog_id: catalog_id.to_string(),
                    principal: request.principal.clone(),
                    resource: record.resource.clone(),
                    permissions: record.permissions.clone(),
                    permissions_with_grant_option: record.permissions_with_grant_option.clone(),
                    condition_expression: request.condition.clone(),
                    last_updated: None,
                    last_updated_by: None,
                };
                grant.written_by(provenance);
                store.permission_grants.insert(record.key.clone(), grant);
                inserted += 1;
            }
        }
    }
    Ok((inserted, updated))
}
