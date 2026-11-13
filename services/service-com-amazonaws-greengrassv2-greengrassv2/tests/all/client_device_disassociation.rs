// SPDX-License-Identifier: CC0-1.0

//! `BatchDisassociateClientDeviceFromCoreDevice`, end to end over the real `restJson1`
//! front door: what removing client devices' cloud-discovery associations answers, what
//! it changes, and what it leaves alone.
//!
//! The rules under test:
//!
//! * the operation resolves its `coreDeviceThingName` against the client-device
//!   ASSOCIATION store alone, so it is served with a 200 for ANY thing name the model
//!   admits — including one no core device has ever reported itself under — and reports
//!   `errorEntries` as a present, empty list: an entry whose association is not there
//!   already leaves the request where it asked to be ("after you disassociate a client
//!   device from a core device, the client device won't be able to use cloud discovery to
//!   retrieve the core device's connectivity information and certificates", the
//!   operation's own documentation), so it is no per-entry failure;
//! * a batch removes EXACTLY the associations its entries name and nothing else: the
//!   other client devices of the same core device keep their association and the instant
//!   it was made, the same client device's association with another core device survives,
//!   and applying the same batch again answers the same and leaves the same state;
//! * a removal touches nothing else this service holds — it registers no core device, and
//!   it neither removes nor disturbs a core device that has reported itself; and
//! * `entries` is a `DisassociateClientDeviceFromCoreDeviceEntryList` of `length 1..100`
//!   (`smithy_query(query="shape",shape="DisassociateClientDeviceFromCoreDeviceEntryList")`),
//!   so a list at either bound is served, a list outside it is refused with a 400
//!   `ValidationException` naming the member and the bound and changes nothing, and
//!   omitting the optional member is a served request that removes nothing.
//!
//! The rows differ along what the rules generalise over: the FORM of the core device
//! thing name (the shared catalog of forms the modelled `length 1..128` admits), the SIZE
//! of a batch (one entry, several, the modelled maximum, one past it, an empty list, no
//! list at all), its CONTENT (entries that are associated, entries that never were, a
//! mixture of the two, the same entry twice) and the ORDER batches are applied in.
//!
//! The harness — the router over one shared engine, the signed call, the body readers,
//! the two batch routes, the association listing and the core-device status report — is
//! the one `tests/all/core_device_fleet.rs` owns, so a Greengrass request is built and
//! read exactly one way across this crate. That sibling suite holds the core-device
//! surface as a whole; the axis here is the removal itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::core_device_fleet::{
    associate, call, disassociate, emulator, error_code, get_core_device, list,
    list_client_devices, list_core_devices, report, text, thing_name_forms, timestamp, Report,
};

/// The most entries one request may carry: the modelled maximum of
/// `DisassociateClientDeviceFromCoreDeviceEntryList` (`length 1..100`).
const MAX_ENTRIES: usize = 100;

/// The longest `IoTThingName` the model admits (`length 1..128`), which both the core
/// device and a client device are named by.
const MAX_THING_NAME: usize = 128;

/// The `CoreDeviceStatus` a reporting device carries here
/// (`smithy_query(query="shape",shape="CoreDeviceStatus")`: `HEALTHY`, `UNHEALTHY`).
const HEALTHY: &str = "HEALTHY";

/// The code a request outside a modelled constraint is refused with, and its status.
const VALIDATION_CODE: &str = "ValidationException";

/// How the boundary opens a refusal that carries one violated constraint.
const VALIDATION_ENVELOPE_PREFIX: &str = "1 validation error detected: ";

/// The thing name the one reporting core device of these suites uses — distinct from
/// every form in the shared catalog, so a row that drives a name the fleet holds no
/// device for stays that way.
const REPORTED_CORE: &str = "disassociation-core";

/// The `message` a `restJson1` error body carries, under either casing the envelope may
/// use.
fn stated(body: &Value) -> String {
    let lower = text(body, "message");
    if lower.is_empty() {
        text(body, "Message")
    } else {
        lower
    }
}

/// The client devices currently associated with `name`, in the order the listing reports
/// them, with the instant each association was made.
async fn associated(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<(String, f64)> {
    let (status, body) = list_client_devices(router, name, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: the association listing must answer 200, got {status} {body}"
    );
    list(&body, "associatedClientDevices")
        .iter()
        .map(|device| {
            (
                text(device, "thingName"),
                timestamp(device, "associationTimestamp"),
            )
        })
        .collect()
}

/// Just the thing names [`associated`] reports.
async fn associated_names(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    associated(router, case, name)
        .await
        .into_iter()
        .map(|(thing_name, _)| thing_name)
        .collect()
}

/// `BatchDisassociateClientDeviceFromCoreDevice` with a RAW request body, for the rows
/// whose `entries` member is not a well-formed list of thing names — the same route the
/// shared harness's `disassociate` addresses.
async fn disassociate_body(
    router: &MultiServiceRouter,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/greengrass/v2/coreDevices/{name}/disassociateClientDevices"),
        serde_json::to_vec(body).expect("a serializable request body"),
    )
    .await
}

/// The batches a removal is driven with against a thing name that holds no association:
/// the sizes and contents the rule generalises over.
fn batches_against_an_empty_name() -> Vec<(&'static str, Vec<String>)> {
    let longest_client_device = "d".repeat(MAX_THING_NAME);
    vec![
        ("one entry", vec!["sensor-1".to_string()]),
        (
            "several entries at once",
            vec![
                "sensor-1".to_string(),
                "sensor-2".to_string(),
                "sensor-3".to_string(),
            ],
        ),
        (
            "the same entry twice in one batch",
            vec!["sensor-1".to_string(), "sensor-1".to_string()],
        ),
        (
            "the shortest and the longest client device name the model admits",
            vec!["c".to_string(), longest_client_device],
        ),
        (
            "the modelled maximum of entries",
            (0..MAX_ENTRIES).map(|index| format!("bulk-{index:03}")).collect(),
        ),
    ]
}

#[tokio::test]
async fn removing_an_association_that_is_not_there_is_an_applied_batch() {
    let (router, engine) = emulator();

    // One name a core device has reported itself under, and the shared catalog of forms
    // no device ever reports under: the rule is the same for both, because the removal is
    // keyed by the THING NAME in the association store rather than by fleet membership.
    report(&engine, &Report::new(REPORTED_CORE, HEALTHY));
    let mut names = vec![(
        "a thing name a core device reported itself under",
        REPORTED_CORE.to_string(),
    )];
    names.extend(thing_name_forms());

    for (name_case, name) in names {
        for (batch_case, batch) in batches_against_an_empty_name() {
            let case = format!("{name_case} / {batch_case}");
            let entries: Vec<&str> = batch.iter().map(String::as_str).collect();
            let (status, body) = disassociate(&router, &name, &entries).await;
            assert_eq!(
                (status, body.clone()),
                (200, json!({ "errorEntries": [] })),
                "case {case:?}: removing an association that is not there must answer 200 \
                 with a present, empty per-entry error channel, got {status} {body}"
            );

            // And it brought nothing into being: the name still holds no association.
            let listed = associated_names(&router, &case, &name).await;
            assert_eq!(
                listed,
                Vec::<String>::new(),
                "case {case:?}: a removal must associate nothing, got {listed:?}"
            );
        }
    }

    // The fleet still holds exactly the device that reported itself: a removal registers
    // no core device, and it removes none either.
    let (status, body) = list_core_devices(&router, "").await;
    let fleet: Vec<String> = list(&body, "coreDevices")
        .iter()
        .map(|device| text(device, "coreDeviceThingName"))
        .collect();
    assert_eq!(
        (status, fleet),
        (200, vec![REPORTED_CORE.to_string()]),
        "case after every removal: the fleet must be the devices that reported \
         themselves, got {status} {body}"
    );
    let (status, body) = get_core_device(&router, REPORTED_CORE).await;
    assert_eq!(
        (status, text(&body, "status")),
        (200, HEALTHY.to_string()),
        "case after every removal: the reported device must still report what it \
         reported, got {status} {body}"
    );
}

/// One removal: the batch to apply, and the client devices that must survive it.
struct RemovalCase {
    label: &'static str,
    remove: Vec<String>,
    remaining: Vec<String>,
}

/// The client devices associated with the core device under test before each removal.
fn associated_before() -> Vec<&'static str> {
    vec![
        "sensor-1",
        "sensor-2",
        "sensor-3",
        "sensor-4",
        "shared-device",
    ]
}

/// The client devices associated with ANOTHER core device, which no removal below names:
/// one of them is associated with both, which is what makes the per-core-device scope of
/// a removal observable.
fn associated_elsewhere() -> Vec<&'static str> {
    vec!["sensor-1", "shared-device"]
}

fn removal_rows() -> Vec<RemovalCase> {
    let owned = |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
    vec![
        RemovalCase {
            label: "one associated entry",
            remove: owned(&["sensor-1"]),
            remaining: owned(&["sensor-2", "sensor-3", "sensor-4", "shared-device"]),
        },
        RemovalCase {
            label: "several associated entries at once",
            remove: owned(&["sensor-1", "sensor-3"]),
            remaining: owned(&["sensor-2", "sensor-4", "shared-device"]),
        },
        RemovalCase {
            label: "the same associated entry twice in one batch",
            remove: owned(&["sensor-2", "sensor-2"]),
            remaining: owned(&["sensor-1", "sensor-3", "sensor-4", "shared-device"]),
        },
        RemovalCase {
            label: "an entry that was never associated",
            remove: owned(&["ghost-1"]),
            remaining: owned(&[
                "sensor-1",
                "sensor-2",
                "sensor-3",
                "sensor-4",
                "shared-device",
            ]),
        },
        RemovalCase {
            label: "a mixture of associated and never-associated entries",
            remove: owned(&["ghost-2", "sensor-4", "ghost-3"]),
            remaining: owned(&["sensor-1", "sensor-2", "sensor-3", "shared-device"]),
        },
        RemovalCase {
            label: "the client device that is associated with another core device too",
            remove: owned(&["shared-device"]),
            remaining: owned(&["sensor-1", "sensor-2", "sensor-3", "sensor-4"]),
        },
        RemovalCase {
            label: "every association at once",
            remove: owned(&[
                "sensor-1",
                "sensor-2",
                "sensor-3",
                "sensor-4",
                "shared-device",
            ]),
            remaining: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_batch_removes_exactly_the_associations_its_entries_name() {
    // The core device the removals are applied to, and the one that must be left alone.
    let target = "removal-target";
    let bystander = "removal-bystander";

    for row in removal_rows() {
        let RemovalCase {
            label: case,
            remove,
            remaining,
        } = row;
        let (router, _engine) = emulator();
        for (name, devices) in [
            (target, associated_before()),
            (bystander, associated_elsewhere()),
        ] {
            let (status, body) = associate(&router, name, &devices).await;
            assert_eq!(
                (status, body.clone()),
                (200, json!({ "errorEntries": [] })),
                "case {case:?}: the setup association on {name} must be applied, got \
                 {status} {body}"
            );
        }
        let before = associated(&router, case, target).await;
        let bystander_before = associated(&router, case, bystander).await;

        let entries: Vec<&str> = remove.iter().map(String::as_str).collect();
        let (status, body) = disassociate(&router, target, &entries).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "errorEntries": [] })),
            "case {case:?}: the removal must answer 200 with an empty per-entry error \
             channel, got {status} {body}"
        );

        // Exactly the named associations are gone, in the listing's own order.
        let after = associated(&router, case, target).await;
        let names: Vec<String> = after.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            names, remaining,
            "case {case:?}: expected exactly {remaining:?} still associated, got {names:?}"
        );

        // A survivor keeps the instant its association was made: a removal re-stamps
        // nothing it did not remove.
        for (name, instant) in &after {
            let was = before
                .iter()
                .find(|(before_name, _)| before_name == name)
                .map(|(_, instant)| *instant)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {name} was associated before the removal")
                });
            assert_eq!(
                *instant, was,
                "case {case:?}: {name} must keep the instant it was associated at, \
                 expected {was}, got {instant}"
            );
        }

        // Another core device's associations are its own, the shared client device
        // included.
        let bystander_after = associated(&router, case, bystander).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {case:?}: the other core device's associations must be untouched, \
             expected {bystander_before:?}, got {bystander_after:?}"
        );

        // Applying the same batch again answers the same and leaves the same state: the
        // removal asks for an end state, so it is idempotent.
        let (status, body) = disassociate(&router, target, &entries).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({ "errorEntries": [] })),
            "case {case:?}: re-applying the batch must answer 200 with an empty per-entry \
             error channel, got {status} {body}"
        );
        let replayed = associated(&router, case, target).await;
        assert_eq!(
            replayed, after,
            "case {case:?}: re-applying the batch must change nothing, expected \
             {after:?}, got {replayed:?}"
        );
    }
}

#[tokio::test]
async fn the_entries_list_is_served_at_its_bounds_and_refused_outside_them() {
    let (router, _engine) = emulator();
    let core = "entries-bounds-core";

    // Exactly the modelled maximum is served, and every entry of it is applied.
    let bulk: Vec<String> = (0..MAX_ENTRIES)
        .map(|index| format!("bulk-{index:03}"))
        .collect();
    let bulk_entries: Vec<&str> = bulk.iter().map(String::as_str).collect();
    let (status, body) = associate(&router, core, &bulk_entries).await;
    assert_eq!(
        status, 200,
        "case the modelled maximum of entries: the setup association must be applied, \
         got {status} {body}"
    );
    let (status, body) = disassociate(&router, core, &bulk_entries).await;
    assert_eq!(
        (status, body.clone()),
        (200, json!({ "errorEntries": [] })),
        "case the modelled maximum of entries: expected 200 with an empty per-entry \
         error channel, got {status} {body}"
    );
    let listed = associated_names(&router, "the modelled maximum of entries", core).await;
    assert_eq!(
        listed,
        Vec::<String>::new(),
        "case the modelled maximum of entries: every entry must be applied, got {listed:?}"
    );

    // One association to hold for the rest of this suite: what the rows below must not
    // disturb.
    let kept = "kept-device";
    let (status, body) = associate(&router, core, &[kept]).await;
    assert_eq!(
        status, 200,
        "case the kept association: the setup association must be applied, got {status} \
         {body}"
    );

    // `entries` is optional, so a request that names none is served — and removes
    // nothing, because it asked for nothing.
    let (status, body) = disassociate_body(&router, core, &json!({})).await;
    assert_eq!(
        (status, body.clone()),
        (200, json!({ "errorEntries": [] })),
        "case no entries member at all: expected 200 with an empty per-entry error \
         channel, got {status} {body}"
    );

    // Outside the modelled bounds the request is refused, naming the member and the
    // bound, and it changes nothing.
    let past_the_maximum: Vec<Value> = (0..=MAX_ENTRIES)
        .map(|index| json!({ "thingName": format!("bulk-{index:03}") }))
        .collect();
    for (case, entries, bound) in [
        (
            "an empty entries list",
            Vec::new(),
            "greater than or equal to 1",
        ),
        (
            "one entry past the modelled maximum",
            past_the_maximum,
            "less than or equal to 100",
        ),
    ] {
        let (status, body) = disassociate_body(&router, core, &json!({ "entries": entries })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            code.contains(VALIDATION_CODE),
            "case {case:?}: expected {VALIDATION_CODE}, got {code:?} {body}"
        );
        // The refusal names the member that was wrong and the bound it had to satisfy,
        // inside the one-error envelope the boundary states; what it renders of the list
        // itself is the framework's, so it is not pinned here.
        let clause =
            format!("at 'entries' failed to satisfy constraint: Member must have length {bound}");
        let actual = stated(&body);
        assert!(
            actual.starts_with(VALIDATION_ENVELOPE_PREFIX) && actual.ends_with(&clause),
            "case {case:?}: expected {VALIDATION_ENVELOPE_PREFIX:?} … {clause:?}, got \
             {actual:?}"
        );
    }

    // Every row above left the one association this core device holds exactly as it was.
    let listed = associated_names(&router, "after the refused requests", core).await;
    assert_eq!(
        listed,
        vec![kept.to_string()],
        "case after the refused requests: the stored association must be untouched, got \
         {listed:?}"
    );
}
