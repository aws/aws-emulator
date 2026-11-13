Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# Outpost$Generation and Outpost$RackScalingType are permanently absent on the premise that "nothing in this slice places hardware", which the same file and state.rs contradict

_Filed: 2026-10-28T10:06:20Z_
_Operation: CreateOutpost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateOutpost` is the only place in the crate that writes `Outpost$Generation` / `Outpost$RackScalingType`, and it writes `None` to both, justified by the comment "nothing in this slice places hardware". That premise is false in this crate: 20 lines further down the SAME handler states that the install is owed by the fulfilment of an order placed on the Outpost, and `crate::state::settle_orders` / `settle_outposts` do exactly that — they call `installed_hardware(...)` and write `record.assets = hardware`, installing a rack (with a `rack_id`) for an Outpost whose `SupportedHardwareType` is `RACK`.

Consequence: an Outpost that this emulator has installed rack hardware on (order placed -> order completed -> `LifeCycleStatus` ACTIVE -> `ListAssets` reports a rack of units) is reported by `GetOutpost` / `ListOutposts` / `CreateOutpost` / `UpdateOutpost` with no `Generation` and no `RackScalingType`, for ever. No path in the crate ever revisits the two members (`grep` finds `generation`/`rack_scaling_type` written on an `Outpost` only at `create_outpost.rs:285-286`), although the data is already in the crate and already reached through an existing helper: the published catalog carries a per-configuration `generation` and `rack_scaling_type` (`CATALOG_ITEMS`, and `CatalogItem$RackScalingType` is projected for `GetCatalogItem`/`ListCatalogItems`), and the crate already resolves an order's line items to their catalog item by SKU in `configured_instance_type_names` and `subscribed_capacity_vcpus`.

So this is not a gap the crate never attempts: it is a gap a comment admits, on a reason the crate's own installation path invalidates.

## Evidence

The omission and its stated reason — `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_outpost.rs:281-286`:

    // `Generation` and `RackScalingType` describe the hardware an order
    // installs; `CreateOutpost` takes neither, and nothing in this slice
    // places hardware, so the emulator reports no value rather than
    // inventing one.
    generation: None,
    rack_scaling_type: None,

The same handler, 15 lines later, contradicting that premise — `src/ops/create_outpost.rs:300-304`:

    // Nothing in this API installs hardware on request, and creating the
    // Outpost orders none: the install that takes it out of the pending
    // status is owed by the fulfilment of an order placed on it
    // (`CreateOrder`), so this call leaves none owed.

The crate DOES place hardware — `src/state.rs:3659-3683` (`settle_outposts`):

    let settled: Vec<(String, Vec<AssetInfo>)> = self
        .outposts
        .iter()
        .filter(|(_, record)| record.active_at.is_some_and(|due| now >= due))
        .map(|(outpost_id, record)| {
            let hardware = match record.assets.is_empty() {
                true => installed_hardware(
                    record.sequence,
                    record.outpost.supported_hardware_type.as_deref(),
                    &self.orderable_instance_types,
                ),
    ...
        record.outpost.life_cycle_status = Some(LIFE_CYCLE_STATUS_ACTIVE.to_string());
    ...
        if !hardware.is_empty() {
            record.assets = hardware;
        }

and `src/state.rs:480-486` says so in prose: "`CreateOutpost` leaves this empty, and the hardware of the Outpost's own configuration is installed once an order placed on it has delivered hardware to the site".

The data and the helper already exist — `src/state.rs:1891-1898` (a catalog configuration carries both facts):

        generation: OUTPOST_GENERATION_1,
    ...
        rack_scaling_type: Some(RACK_SCALING_TYPE_SINGLE_RACK),

`src/state.rs:2168-2170` projects it onto the published item (`rack_scaling_type: config.rack_scaling_type.map(...)`), and `src/state.rs:3039-3046` is the crate already walking an order's line items to their catalog item:

        for item in record.order.line_items.iter().flatten() {
            let Some(catalog_item) = item
                .catalog_item_id
                .as_deref()
                .and_then(|catalog_item_id| self.catalog_item(catalog_item_id))

Test coverage: no test pins the two members on an Outpost — `grep -n 'Generation|RackScalingType' tests/` matches only `tests/all/get_catalog_item.rs`, `tests/all/catalog_items.rs` and `tests/all/orderable_instance_types.rs` (catalog and orderable-instance-type assertions), while `tests/all/outpost_hardware_installation.rs` and `tests/all/outpost_order_driven_lifecycle.rs` assert the installed assets and the lifecycle status without ever looking at `Generation` / `RackScalingType`.

## Steps to Reproduce

1. CreateSite, then CreateOutpost with SupportedHardwareType=RACK; note GetOutpost reports no Generation and no RackScalingType (correct at this point: nothing installed).
2. CreateOrder on that Outpost with a line item naming a RACK catalog item from ListCatalogItems (one whose published RackScalingType is SINGLE_RACK or MULTI_RACK).
3. Poll GetOrder until it reports COMPLETED, then GetOutpost: LifeCycleStatus is ACTIVE and ListAssets on the Outpost reports an installed rack of units, so hardware HAS been placed.
4. GetOutpost again: Generation and RackScalingType are still absent, and no later call can ever set them — although the catalog item the order named publishes a RackScalingType and carries a generation (state.rs:1891-1898, 2168-2170) which the crate already looks up by SKU for the same order's line items (state.rs:3039-3046).
