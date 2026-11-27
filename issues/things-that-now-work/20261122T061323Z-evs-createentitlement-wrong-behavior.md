Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: b20300340ddfd95fa2f862f6c06bba5164a73eba

# entitlementType is part of an entitlement's identity for DeleteEntitlement and ListVmEntitlements, but not for CreateEntitlement's store key or its idempotent repeat

_Filed: 2026-11-22T06:13:23Z_
_Operation: CreateEntitlement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b20300340ddfd95fa2f862f6c06bba5164a73eba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate gives two contradictory answers to "what identifies one entitlement".

`crate::state::entitlement_key` is `(connectorId, vmId)` and its doc asserts that all three entitlement operations address an entitlement by that pair alone. `CreateEntitlement` relies on exactly that: it looks the key up and, on `Entry::Occupied`, reports the stored record AS the entitlement the call created, without comparing the stored `entitlement_type` against `input.entitlement_type`.

But `DeleteEntitlement` and `ListVmEntitlements` both treat `entitlementType` as a third selector of the addressing, each with an explicit comment saying so: a record of another type "is not the one this call names", and the listing filters on `entitlement.entitlement_type == input.entitlement_type`. Under the create's reading a record of type X occupies the (connector, VM) slot for every type, so a create naming type Y would answer 200 with `type: X` and mint no entitlement of type Y — while the delete addressed to type Y would then find nothing to remove.

`EntitlementType` currently declares one value, so no request can reach the divergence today; the defect is that the identity rule is spelled two incompatible ways inside one crate, and whichever reading is intended, one of the two comments or the key is wrong.

## Evidence

The key, and the doc that asserts all three operations use it — `services/.../src/state.rs:1005-1016`:
```
/// The store key of one entitlement inside its environment: the connector it was
/// created through and the virtual machine it entitles.
///
/// That pair is what identifies an entitlement — `CreateEntitlement`,
/// `ListVmEntitlements` and `DeleteEntitlement` all address entitlements by
/// `connectorId` plus `vmIds` ...
pub fn entitlement_key(connector_id: &str, vm_id: &str) -> String {
    format!("{connector_id}/{vm_id}")
}
```
The create's idempotent repeat, which never looks at the type — `services/.../src/ops/create_entitlement.rs:292-309`:
```
            let key = crate::state::entitlement_key(&input.connector_id, vm_id);
            let entitlement = match environment.entitlements.entry(key) {
                std::collections::btree_map::Entry::Vacant(slot) => { ... }
                // The operation is `@idempotent`: a VM already entitled through this
                // connector keeps the record it has, and the call reports that record
                // rather than creating a second one.
                std::collections::btree_map::Entry::Occupied(taken) => taken.into_mut(),
            };
            entitlements.push(entitlement.to_wire(&environment_id));
```
The sibling that says the opposite — `services/.../src/ops/delete_entitlement.rs:263-273`:
```
            let key = crate::state::entitlement_key(&input.connector_id, vm_id);
            // `entitlementType` selects WHICH entitlement of that pair the request
            // addresses, exactly as it selects the records `ListVmEntitlements` reports,
            // so a record of another type is not the one this call names.
            let addressed = environment
                .entitlements
                .get(&key)
                .is_some_and(|entitlement| entitlement.entitlement_type == input.entitlement_type);
            if !addressed {
                continue;
            }
```
and the listing that filters on it — `services/.../src/ops/list_vm_entitlements.rs:266-269`:
```
            .filter(|(_, entitlement)| {
                entitlement.connector_id == input.connector_id
                    && entitlement.entitlement_type == input.entitlement_type
            });
```
Test coverage of the surrounding behaviour: `tests/all/entitlements.rs`, `tests/all/delete_entitlement.rs`, `tests/all/entitlement_vm_id_identity.rs` (which pins the (connector, VM) identity) — none of them exercises two entitlement types on one pair, because the model declares one.

## Steps to Reproduce

1. Read `crate::state::entitlement_key` (state.rs:1005-1016): the key — and therefore the create's idempotency — is `(connectorId, vmId)` with no entitlement type.
2. Read `CreateEntitlement`'s `Entry::Occupied` arm (create_entitlement.rs:304-308): the stored record is returned unconditionally and reported as the entitlement the call created.
3. Read `DeleteEntitlement` (delete_entitlement.rs:263-273) and `ListVmEntitlements` (list_vm_entitlements.rs:266-269): both require `entitlement_type == input.entitlement_type` before a record counts as addressed.
4. The two readings disagree: under the create's key, one record of type X occupies the pair for every type; under the delete's and the list's, a record of type X is not addressed by a request naming type Y. Were a second `EntitlementType` value modelled, a `CreateEntitlement` for type Y on a pair already holding type X would answer 200 reporting `type: X`, store no type-Y entitlement, and a subsequent `DeleteEntitlement` for type Y would report an empty `entitlements` list.
