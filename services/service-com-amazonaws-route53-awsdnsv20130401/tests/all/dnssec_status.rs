// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the hosted zone's DNSSEC signing state:
//! `GetDNSSEC`, `EnableHostedZoneDNSSEC` and `DisableHostedZoneDNSSEC`.
//!
//! The rule under test: one piece of state — whether Route 53 signs a given
//! hosted zone, over the key-signing keys that zone holds — is REPORTED by the
//! DNSSEC read (`ServeSignature` is `SIGNING` exactly while signing is enabled
//! for that zone and `NOT_SIGNING` while it is not, with one `KeySigningKey`
//! element per KSK the zone holds), turned on by the enable (over a KSK in
//! `ACTIVE` status, and refused when the zone has none) and turned off by the
//! disable (refused when the zone holds no DNSSEC resource at all). Every one of
//! them answers `NoSuchHostedZone` for an id no zone in the store carries, and
//! the read, being a report, leaves the store exactly as it was.
//!
//! One zone is one row of that class, so the tables below span it: zones
//! differing in label count, in public/private, in record population and in
//! their position in the store all report their OWN state; the signing flag and
//! the key-signing keys are read back over several counts (none, one, three) and
//! statuses rather than the single case a recording happened to carry.
//!
//! The tables below put a zone into a signing state by WRITING the store the
//! handlers read, which is what `seed_dnssec` does: a seeded key is the only way
//! to reach a status no API operation creates (`INTERNAL_FAILURE`) and to hold a
//! zone's key population fixed independently of the customer managed keys a
//! create derives one from — `CreateKeySigningKey` is covered end to end by the
//! sibling `key_signing_keys` suite. Nothing else is seeded: signing is turned on
//! and off through the API, and the assertions stay end-to-end on the
//! HTTP/restXml responses a Route 53 client sees, through the same harness the
//! hosted-zone suites use.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::shared_types::KeySigningKey;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    service_state, SERVE_SIGNATURE_NOT_SIGNING, SERVE_SIGNATURE_SIGNING,
};

use crate::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_ok, router, RecordRow, Reply,
};
use crate::vpc_associations::create_private_zone;

/// A router with this service mounted on a fresh engine, PLUS the engine handle.
///
/// The sibling harness's `router()` keeps its engine to itself, which is all a
/// test driving only the API needs; a test that has to put a zone into a state
/// no implemented operation can reach yet needs the store as well.
fn router_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// `GetDNSSEC` for one hosted zone.
///
/// `pub(crate)` so the sibling key-signing-key suite reads a zone's DNSSEC state
/// back through the SAME call rather than growing a second copy of it.
pub(crate) async fn get_dnssec(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/dnssec"),
        "",
    )
    .await
}

/// `EnableHostedZoneDNSSEC` for one hosted zone.
///
/// `pub(crate)` for the same reason [`get_dnssec`] is.
pub(crate) async fn enable_dnssec(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/enable-dnssec"),
        "",
    )
    .await
}

/// `DisableHostedZoneDNSSEC` for one hosted zone.
///
/// `pub(crate)` for the same reason [`get_dnssec`] is.
pub(crate) async fn disable_dnssec(router: &MultiServiceRouter, zone_id: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/disable-dnssec"),
        "",
    )
    .await
}

/// Assert a DNSSEC write was served and that the `ChangeInfo` it answered is a
/// change a caller can poll: a `/change/C…` id in one of the two modelled
/// `ChangeStatus` values.
fn assert_served_change(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 (501 = still scaffolded), got {} with body {}",
        reply.status, reply.body
    );
    let id = reply.first("ChangeInfo/Id").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry ChangeInfo/Id, got body {}",
            reply.body
        )
    });
    let bare = id.strip_prefix("/change/").unwrap_or_else(|| {
        panic!("case {case:?}: a change id is reported behind /change/, got {id:?}")
    });
    assert!(
        bare.starts_with('C')
            && bare.len() == 15
            && bare[1..]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "case {case:?}: a minted change id is C plus 14 uppercase alphanumerics, got {bare:?}"
    );
    let status = reply.first("ChangeInfo/Status").unwrap_or_else(|| {
        panic!(
            "case {case:?}: the reply must carry ChangeInfo/Status, got body {}",
            reply.body
        )
    });
    // The modelled `ChangeStatus` values
    // (smithy_query(query="shape",shape="ChangeStatus")).
    assert!(
        status == "PENDING" || status == "INSYNC",
        "case {case:?}: a change is PENDING or INSYNC, got {status:?}"
    );
}

/// The `ServeSignature` a successful DNSSEC read reported.
fn reported_signature(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetDNSSEC (501 = still scaffolded), \
         got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("Status/ServeSignature")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry Status/ServeSignature, got body {}",
                reply.body
            )
        })
        .to_string()
}

/// The `Name` of every key-signing key a DNSSEC read reported, in document order.
fn reported_ksk_names(reply: &Reply) -> Vec<String> {
    reply
        .all("KeySigningKeys/member/Name")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The text of one member of every reported key-signing key, in document order.
fn reported_ksk_field(reply: &Reply, member: &str) -> Vec<String> {
    reply
        .all(&format!("KeySigningKeys/member/{member}"))
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The `<Error><Message>` text of a refusal.
fn error_message(reply: &Reply) -> &str {
    reply.first("Error/Message").unwrap_or("")
}

/// A refusal that is about the zone the request named reports WHICH zone, built
/// from the request, so a caller driving several zones can tell them apart.
///
/// `InvalidKeySigningKeyStatus` is about a key rather than the zone, and no
/// published rule words it from the request, so it is exempt.
fn assert_refusal_names_the_zone(case: &str, code: &str, zone: &str, reply: &Reply) {
    if code == "InvalidKeySigningKeyStatus" {
        return;
    }
    let message = error_message(reply);
    assert!(
        message.contains(zone),
        "case {case:?}: the {code} refusal must name the zone the request asked about \
         ({zone}), got {message:?}"
    );
}

/// A key-signing key as `CreateKeySigningKey` persists one.
///
/// The constants are the model's own: `KeySigningKey$Flag` — "For key-signing
/// key (KSK), this value is always 257" — and the algorithm members, whose
/// documentation points at RFC-8624 Section 3.1 (signing) and 3.3 (digest);
/// Route 53 signs with ECDSAP256SHA256 (algorithm 13) and publishes a SHA-256
/// (digest type 2) delegation signer.
fn key_signing_key(name: &str, status: &str) -> KeySigningKey {
    KeySigningKey {
        name: Some(name.to_string()),
        kms_arn: Some(format!(
            "arn:aws:kms:us-east-1:123456789012:key/{name}-00000000"
        )),
        flag: 257,
        signing_algorithm_mnemonic: Some("ECDSAP256SHA256".to_string()),
        signing_algorithm_type: 13,
        digest_algorithm_mnemonic: Some("SHA-256".to_string()),
        digest_algorithm_type: 2,
        key_tag: 44_321,
        digest_value: Some("3F7A1C".to_string()),
        public_key: Some("Zm9vYmFyCg==".to_string()),
        ds_record: Some(format!("44321 13 2 3F7A1C {name}")),
        dnskey_record: Some("257 3 13 Zm9vYmFyCg==".to_string()),
        status: Some(status.to_string()),
        status_message: None,
        created_date: None,
        last_modified_date: None,
    }
}

/// Put `zone_id` into the DNSSEC state the row asks for, by writing the store the
/// handler reads.
///
/// Stands in for a key population no create reaches: a status only a failed
/// request leaves, and a fixed set of keys. `EnableHostedZoneDNSSEC` and
/// `CreateKeySigningKey` are driven through the API by the suites that cover
/// them.
fn seed_dnssec(
    engine: &engine::InMemoryEngine,
    zone_id: &str,
    signing: bool,
    ksks: &[(&str, &str)],
) {
    let state = service_state(engine).expect("the Route 53 store must be reachable");
    let mut store = state.lock();
    let zone = store
        .zones
        .get_mut(zone_id)
        .unwrap_or_else(|| panic!("the zone {zone_id} must be in the store"));
    zone.dnssec_signing_enabled = signing;
    for (name, status) in ksks {
        zone.key_signing_keys
            .insert((*name).to_string(), key_signing_key(name, status));
    }
}

#[tokio::test]
async fn a_zone_nobody_enabled_signing_for_reports_not_signing_and_no_keys() {
    // The rows differ along what a zone can differ in at creation: the number of
    // labels in its name, whether it is public or private, whether it holds
    // records beyond the apex defaults, and where it sits among the zones the
    // store holds. None of that is DNSSEC state, so every row reports the same
    // unsigned status — and reports it with an EMPTY key list, not an absent one.
    let router = router();

    let plain = create_zone_ok(&router, "example.com", "dnssec-unsigned-0").await;
    let deep = create_zone_ok(&router, "a.b.c.deep.example.net", "dnssec-unsigned-1").await;
    let dotted = create_zone_ok(&router, "trailing.example.org.", "dnssec-unsigned-2").await;
    let private =
        create_private_zone(&router, "internal.example", "dnssec-unsigned-3", "vpc-1a2b3c4d").await;

    let with_records = create_zone_ok(&router, "records.example.com", "dnssec-unsigned-4").await;
    let row = RecordRow {
        name: "www.records.example.com",
        rr_type: "A",
        ttl: 300,
        values: &["192.0.2.10"],
    };
    let changed = call(
        &router,
        "POST",
        &format!("/2013-04-01/hostedzone/{with_records}/rrset"),
        &change_batch_body(&change_element("CREATE", &row), None),
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "expected 200 from ChangeResourceRecordSets, got {} with body {}",
        changed.status, changed.body
    );

    let cases = [
        ("a two-label public zone", plain),
        ("a five-label public zone", deep),
        ("a name given with the trailing dot", dotted),
        ("a private zone", private),
        ("a zone holding a record beyond the apex defaults", with_records),
    ];
    for (case, zone) in cases {
        let reply = get_dnssec(&router, &zone).await;
        let signature = reported_signature(case, &reply);
        assert_eq!(
            signature, SERVE_SIGNATURE_NOT_SIGNING,
            "case {case:?}: a zone nobody enabled signing for must report \
             {SERVE_SIGNATURE_NOT_SIGNING}, got {signature:?}"
        );
        let names = reported_ksk_names(&reply);
        assert!(
            names.is_empty(),
            "case {case:?}: a zone no KSK was created for must report none, got {names:?}"
        );
        assert!(
            reply.first("KeySigningKeys").is_some() || reply.body.contains("KeySigningKeys"),
            "case {case:?}: KeySigningKeys is a required member, so the container must be \
             in the response, got body {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_status_reported_is_the_zone_s_own_persisted_dnssec_state() {
    // The rule generalises over the signing flag and over the NUMBER of
    // key-signing keys, and the two are independent: Route 53 lets a KSK exist in
    // a zone it is not signing, and the signing status is the zone's own, not its
    // neighbours'. One row per combination, all in ONE store, each read back
    // through the API.
    struct Row {
        case: &'static str,
        name: &'static str,
        caller_reference: &'static str,
        signing: bool,
        ksks: &'static [(&'static str, &'static str)],
    }
    let rows = [
        Row {
            case: "unsigned, no key-signing key",
            name: "plain.example.com",
            caller_reference: "dnssec-state-0",
            signing: false,
            ksks: &[],
        },
        Row {
            case: "unsigned, one key-signing key created",
            name: "key-only.example.com",
            caller_reference: "dnssec-state-1",
            signing: false,
            ksks: &[("ksk_one", "ACTIVE")],
        },
        Row {
            case: "signing with one key-signing key",
            name: "signed.example.net",
            caller_reference: "dnssec-state-2",
            signing: true,
            ksks: &[("ksk_alpha", "ACTIVE")],
        },
        Row {
            case: "signing with three key-signing keys",
            name: "multi.example.org",
            caller_reference: "dnssec-state-3",
            signing: true,
            // Inserted out of order on purpose: the report is in Name order.
            ksks: &[
                ("ksk_zulu", "ACTIVE"),
                ("ksk_mike", "INACTIVE"),
                ("ksk_alpha", "ACTIVE"),
            ],
        },
        Row {
            case: "unsigned again, three key-signing keys",
            name: "unsigned-multi.example.com",
            caller_reference: "dnssec-state-4",
            signing: false,
            ksks: &[
                ("ksk_bravo", "INACTIVE"),
                ("ksk_charlie", "ACTIVE"),
                ("ksk_delta", "INACTIVE"),
            ],
        },
    ];

    let (router, engine) = router_with_store();
    let mut zones = Vec::new();
    for row in &rows {
        let zone = create_zone_ok(&router, row.name, row.caller_reference).await;
        seed_dnssec(&engine, &zone, row.signing, row.ksks);
        zones.push(zone);
    }

    for (row, zone) in rows.iter().zip(&zones) {
        let case = row.case;
        let reply = get_dnssec(&router, zone).await;
        let signature = reported_signature(case, &reply);
        let expected = if row.signing {
            SERVE_SIGNATURE_SIGNING
        } else {
            SERVE_SIGNATURE_NOT_SIGNING
        };
        assert_eq!(
            signature, expected,
            "case {case:?}: the reported status must be the zone's own, \
             expected {expected}, got {signature:?}"
        );

        let mut expected_names: Vec<String> =
            row.ksks.iter().map(|(name, _)| (*name).to_string()).collect();
        expected_names.sort_unstable();
        let names = reported_ksk_names(&reply);
        assert_eq!(
            names, expected_names,
            "case {case:?}: every key-signing key the zone holds must be reported once, \
             in Name order, got {names:?}"
        );

        // The whole KSK comes out of the store, not just its name: the members a
        // client reads to publish a DS record are reported for every key.
        let expected_statuses: Vec<String> = row
            .ksks
            .iter()
            .map(|(name, status)| ((*name).to_string(), (*status).to_string()))
            .collect::<std::collections::BTreeMap<String, String>>()
            .into_values()
            .collect();
        let statuses = reported_ksk_field(&reply, "Status");
        assert_eq!(
            statuses, expected_statuses,
            "case {case:?}: each reported key must carry its stored status, got {statuses:?}"
        );
        let flags = reported_ksk_field(&reply, "Flag");
        assert_eq!(
            flags,
            vec!["257".to_string(); expected_names.len()],
            "case {case:?}: a KSK's Flag is 257, got {flags:?}"
        );
        let ds_records = reported_ksk_field(&reply, "DSRecord");
        let expected_ds: Vec<String> = expected_names
            .iter()
            .map(|name| format!("44321 13 2 3F7A1C {name}"))
            .collect();
        assert_eq!(
            ds_records, expected_ds,
            "case {case:?}: each reported key must carry its stored DS record, got {ds_records:?}"
        );
    }
}

#[tokio::test]
async fn signing_state_is_not_shared_between_zones_or_stores() {
    // Two stores, so the state is per-ZONE and per-store rather than per-name: a
    // zone signed in one engine says nothing about the same name in another.
    let (first_router, first_engine) = router_with_store();
    let (second_router, _second_engine) = router_with_store();

    let signed = create_zone_ok(&first_router, "shared.example.com", "dnssec-shared-0").await;
    let unsigned = create_zone_ok(&first_router, "other.example.com", "dnssec-shared-1").await;
    seed_dnssec(&first_engine, &signed, true, &[("ksk_only", "ACTIVE")]);

    let elsewhere = create_zone_ok(&second_router, "shared.example.com", "dnssec-shared-0").await;

    let cases: [(&str, &MultiServiceRouter, &String, &str, usize); 3] = [
        (
            "the signed zone",
            &first_router,
            &signed,
            SERVE_SIGNATURE_SIGNING,
            1,
        ),
        (
            "its neighbour in the same store",
            &first_router,
            &unsigned,
            SERVE_SIGNATURE_NOT_SIGNING,
            0,
        ),
        (
            "the same name in a separate store",
            &second_router,
            &elsewhere,
            SERVE_SIGNATURE_NOT_SIGNING,
            0,
        ),
    ];
    for (case, router, zone, expected, key_count) in cases {
        let reply = get_dnssec(router, zone).await;
        let signature = reported_signature(case, &reply);
        assert_eq!(
            signature, expected,
            "case {case:?}: expected {expected}, got {signature:?}"
        );
        let names = reported_ksk_names(&reply);
        assert_eq!(
            names.len(),
            key_count,
            "case {case:?}: expected {key_count} key-signing key(s), got {names:?}"
        );
    }
}

#[tokio::test]
async fn an_id_no_hosted_zone_carries_is_refused_as_no_such_hosted_zone() {
    // The rows differ along the ways an id can fail to name a zone: never minted,
    // minted and then deleted, a real id spelled in another case, an id of
    // another resource kind, and the shortest and longest ids the modelled
    // `ResourceId` (len 0..32) can carry.
    let router = router();
    let live = create_zone_ok(&router, "live.example.com", "dnssec-missing-live").await;

    let doomed = create_zone_ok(&router, "doomed.example.com", "dnssec-missing-doomed").await;
    let deleted = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{doomed}"),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "deleting an empty zone must answer 200, got {} with body {}",
        deleted.status, deleted.body
    );

    let lowercased = live.to_lowercase();
    assert_ne!(
        lowercased, live,
        "the case-folded id must differ from the real one for this row to say anything"
    );

    let cases: Vec<(&str, String)> = vec![
        ("a well-formed id no zone was minted under", "Z1D633PJN98FT9".to_string()),
        ("the id of a zone that was deleted", doomed.clone()),
        ("a live zone's id spelled in lower case", lowercased),
        ("a change id rather than a zone id", "C2682N5HXP0BZ4".to_string()),
        ("a one-character id", "Z".to_string()),
        // The longest `ResourceId` the model allows (len 0..32); one longer is
        // refused before the handler, as a constraint violation.
        ("a 32-character id", "Z".repeat(32)),
    ];
    for (case, id) in cases {
        let reply = get_dnssec(&router, &id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?} ({id}): an id no zone carries must answer 404, \
             got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHostedZone",
            "case {case:?} ({id}): expected the NoSuchHostedZone code, got body {}",
            reply.body
        );
        let message = error_message(&reply);
        assert!(
            message.contains(&id),
            "case {case:?}: the refusal must name the id the request asked about, \
             got {message:?}"
        );
    }

    // And the live zone is still readable: a refusal says nothing about it.
    let reply = get_dnssec(&router, &live).await;
    let signature = reported_signature("the live zone after the refusals", &reply);
    assert_eq!(
        signature, SERVE_SIGNATURE_NOT_SIGNING,
        "the live zone must still report its state, got {signature:?}"
    );
}

#[tokio::test]
async fn reading_the_dnssec_status_changes_nothing_a_later_caller_sees() {
    // The operation is a report: repeating it must not move the status, the key
    // list, the zone or its records. The rows are the reads themselves — the
    // first, a repeat, one taken after a refused read of another id, and one
    // taken after a record change.
    let (router, engine) = router_with_store();
    let zone = create_zone_ok(&router, "stable.example.com", "dnssec-stable-0").await;
    seed_dnssec(&engine, &zone, false, &[("ksk_stable", "ACTIVE")]);
    assert_served_change("enabling signing over the seeded key", &enable_dnssec(&router, &zone).await);

    let baseline = get_dnssec(&router, &zone).await;
    let expected_signature = reported_signature("the first read", &baseline);
    assert_eq!(
        expected_signature, SERVE_SIGNATURE_SIGNING,
        "the seeded zone must report {SERVE_SIGNATURE_SIGNING}, got {expected_signature:?}"
    );
    let expected_names = reported_ksk_names(&baseline);

    let refused = get_dnssec(&router, "Z1D633PJN98FT9").await;
    assert_eq!(
        refused.status, 404,
        "the refusal row needs a 404, got {} with body {}",
        refused.status, refused.body
    );

    let row = RecordRow {
        name: "www.stable.example.com",
        rr_type: "A",
        ttl: 60,
        values: &["192.0.2.20"],
    };
    let changed = call(
        &router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        &change_batch_body(&change_element("CREATE", &row), None),
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "expected 200 from ChangeResourceRecordSets, got {} with body {}",
        changed.status, changed.body
    );

    for case in [
        "a repeat of the first read",
        "a read after a refused one",
        "a read after a record change",
    ] {
        let reply = get_dnssec(&router, &zone).await;
        let signature = reported_signature(case, &reply);
        assert_eq!(
            signature, expected_signature,
            "case {case:?}: the reported status must not move, got {signature:?}"
        );
        let names = reported_ksk_names(&reply);
        assert_eq!(
            names, expected_names,
            "case {case:?}: the reported key list must not move, got {names:?}"
        );
    }

    // And the zone itself is untouched: the reads neither created nor destroyed
    // one, and the record written between them is still the only added record.
    let listed = call(&router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListHostedZones, got {} with body {}",
        listed.status, listed.body
    );
    let zone_ids = listed.all("HostedZone/Id").len();
    assert_eq!(
        zone_ids, 1,
        "reading the DNSSEC status must not create or destroy a zone, \
         ListHostedZones reports {zone_ids}"
    );
    let records = call(
        &router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        "",
    )
    .await;
    let names = records.all("ResourceRecordSet/Name");
    assert!(
        names.contains(&"www.stable.example.com."),
        "the record written between the reads must still be there, got {names:?}"
    );
}

#[tokio::test]
async fn signing_is_turned_on_over_an_active_key_and_off_again() {
    // The round trip: the enable writes the signing state, the read reports it,
    // the disable writes it back, and the read reports that. Each row is one call
    // in the sequence, with the status the reader must report after it — and the
    // repeats are there because both writes promise an END STATE ("enables /
    // disables DNSSEC signing in a specific hosted zone"), so applying one twice
    // leaves the same state rather than refusing.
    let (router, engine) = router_with_store();
    let zone = create_zone_ok(&router, "signed.example.com", "dnssec-roundtrip-0").await;
    // Stands in for CreateKeySigningKey, still a scaffold: Route 53 signs with a
    // KSK, so a zone needs one before signing can start.
    seed_dnssec(&engine, &zone, false, &[("ksk_roundtrip", "ACTIVE")]);

    let before = get_dnssec(&router, &zone).await;
    let signature = reported_signature("before the enable", &before);
    assert_eq!(
        signature, SERVE_SIGNATURE_NOT_SIGNING,
        "a zone with a key but no enable must still report \
         {SERVE_SIGNATURE_NOT_SIGNING}, got {signature:?}"
    );

    enum Step {
        Enable,
        Disable,
    }
    let steps = [
        ("the first enable", Step::Enable, SERVE_SIGNATURE_SIGNING),
        ("a repeated enable", Step::Enable, SERVE_SIGNATURE_SIGNING),
        ("the first disable", Step::Disable, SERVE_SIGNATURE_NOT_SIGNING),
        (
            "a repeated disable, the zone still holding its key",
            Step::Disable,
            SERVE_SIGNATURE_NOT_SIGNING,
        ),
        ("an enable after the disable", Step::Enable, SERVE_SIGNATURE_SIGNING),
    ];
    for (case, step, expected) in steps {
        let reply = match step {
            Step::Enable => enable_dnssec(&router, &zone).await,
            Step::Disable => disable_dnssec(&router, &zone).await,
        };
        assert_served_change(case, &reply);

        let read = get_dnssec(&router, &zone).await;
        let signature = reported_signature(case, &read);
        assert_eq!(
            signature, expected,
            "case {case:?}: after this call the zone must report {expected}, got {signature:?}"
        );
        // "This action does not deactivate any key-signing keys (KSKs) that are
        // active in the hosted zone": neither write touches the key list.
        let names = reported_ksk_names(&read);
        assert_eq!(
            names,
            vec!["ksk_roundtrip".to_string()],
            "case {case:?}: the zone's key-signing keys must be left alone, got {names:?}"
        );
        let statuses = reported_ksk_field(&read, "Status");
        assert_eq!(
            statuses,
            vec!["ACTIVE".to_string()],
            "case {case:?}: a key's status must be left alone, got {statuses:?}"
        );
    }
}

#[tokio::test]
async fn a_dnssec_write_the_zone_s_state_does_not_allow_is_refused_and_changes_nothing() {
    // The rows differ along the zone state each write is refused for: what the
    // zone holds (nothing, a key that is not signing-capable, a key left in
    // error) and whether the id names a zone at all. Each row asserts the status
    // code, the wire error code, and that the refusal moved nothing — a refused
    // write must not leave the zone signing.
    struct Row {
        case: &'static str,
        name: &'static str,
        caller_reference: &'static str,
        ksks: &'static [(&'static str, &'static str)],
        /// Whether signing is on before the writes are attempted.
        signing: bool,
        /// The expected `(status, code)` of the enable, or `None` when the row
        /// expects the enable to be served.
        enable_refusal: Option<(u16, &'static str)>,
        /// The expected `(status, code)` of the disable, or `None` when the row
        /// expects it to be served.
        disable_refusal: Option<(u16, &'static str)>,
    }
    let rows = [
        Row {
            case: "a zone with no DNSSEC at all",
            name: "bare.example.com",
            caller_reference: "dnssec-refusal-0",
            ksks: &[],
            signing: false,
            // Route 53 signs with a KSK, and this zone has none.
            enable_refusal: Some((400, "KeySigningKeyWithActiveStatusNotFound")),
            // "The hosted zone doesn't have any DNSSEC resources."
            disable_refusal: Some((400, "DNSSECNotFound")),
        },
        Row {
            case: "a zone whose only key is INACTIVE",
            name: "inactive.example.com",
            caller_reference: "dnssec-refusal-1",
            ksks: &[("ksk_inactive", "INACTIVE")],
            signing: false,
            // "The KSK is not being used for signing", so there is no ACTIVE one.
            enable_refusal: Some((400, "KeySigningKeyWithActiveStatusNotFound")),
            // The zone DOES hold a DNSSEC resource, so there is something to
            // disable: the disable is served.
            disable_refusal: None,
        },
        Row {
            case: "a zone holding a key left in INTERNAL_FAILURE",
            name: "broken.example.com",
            caller_reference: "dnssec-refusal-2",
            ksks: &[
                ("ksk_broken", "INTERNAL_FAILURE"),
                ("ksk_usable", "ACTIVE"),
            ],
            signing: false,
            // "another KSK has the status INTERNAL_FAILURE" — the problem has to
            // be corrected before signing can be worked with.
            enable_refusal: Some((400, "InvalidKeySigningKeyStatus")),
            // Disabling is how the documentation says that state is corrected,
            // so it is served.
            disable_refusal: None,
        },
    ];

    let (router, engine) = router_with_store();
    for row in &rows {
        let case = row.case;
        let zone = create_zone_ok(&router, row.name, row.caller_reference).await;
        seed_dnssec(&engine, &zone, row.signing, row.ksks);

        let reply = enable_dnssec(&router, &zone).await;
        match row.enable_refusal {
            Some((status, code)) => {
                assert_eq!(
                    reply.status, status,
                    "case {case:?}: the enable must answer {status}, got {} with body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    code,
                    "case {case:?}: the enable must answer the {code} code, got body {}",
                    reply.body
                );
                assert_refusal_names_the_zone(case, code, &zone, &reply);
            }
            None => assert_served_change(case, &reply),
        }

        let reply = disable_dnssec(&router, &zone).await;
        match row.disable_refusal {
            Some((status, code)) => {
                assert_eq!(
                    reply.status, status,
                    "case {case:?}: the disable must answer {status}, got {} with body {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    code,
                    "case {case:?}: the disable must answer the {code} code, got body {}",
                    reply.body
                );
                assert_refusal_names_the_zone(case, code, &zone, &reply);
            }
            None => assert_served_change(case, &reply),
        }

        // A refused write leaves the zone unsigned, and a served disable leaves
        // it unsigned too: either way the zone is not signing afterwards.
        let read = get_dnssec(&router, &zone).await;
        let signature = reported_signature(case, &read);
        assert_eq!(
            signature, SERVE_SIGNATURE_NOT_SIGNING,
            "case {case:?}: the zone must not be left signing, got {signature:?}"
        );
        let names = reported_ksk_names(&read);
        assert_eq!(
            names.len(),
            row.ksks.len(),
            "case {case:?}: the zone's key list must be untouched, got {names:?}"
        );
    }

    // And an id no zone carries is a 404 from both writes, exactly as from the
    // read — a write never invents the zone it was asked about.
    for id in ["Z1D633PJN98FT9", "C2682N5HXP0BZ4", "Z"] {
        for (call_name, reply) in [
            ("EnableHostedZoneDNSSEC", enable_dnssec(&router, id).await),
            ("DisableHostedZoneDNSSEC", disable_dnssec(&router, id).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {id:?}: {call_name} must answer 404 for an unknown zone, \
                 got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHostedZone",
                "case {id:?}: {call_name} must answer the NoSuchHostedZone code, got body {}",
                reply.body
            );
            let message = error_message(&reply);
            assert!(
                message.contains(id),
                "case {id:?}: {call_name}'s refusal must name the id the request asked about, \
                 got {message:?}"
            );
        }
    }
}
