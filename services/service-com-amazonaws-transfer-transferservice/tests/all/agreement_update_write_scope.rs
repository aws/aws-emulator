// SPDX-License-Identifier: CC0-1.0

//! WHICH agreement one `UpdateAgreement` writes, and which refusal a request that breaks
//! more than one rule at once is answered with.
//!
//! The sibling suite `agreement_updates.rs` pins WHAT an update persists onto the record
//! it addresses. These are the two rules it does not cover:
//!
//! 1. WRITE SCOPE: an update writes exactly the `(ServerId, AgreementId)` record it
//!    addresses. Every other agreement — another agreement of the same server, and one of
//!    another server — reads back exactly as it stood before the call, so one partner's
//!    configuration cannot be moved by a write aimed at another. The rows differ along
//!    which record of several is addressed, which server holds it, the directory
//!    configuration each record carries, and which members the update names.
//! 2. REFUSAL ORDER, and atomicity: a request that breaks several rules at once is
//!    answered for the one the service decides FIRST — the documented either/or over the
//!    two directory configurations is decided before anything is looked up, and the
//!    addressed pair is resolved before a profile reference the request carries — and a
//!    refused update leaves every stored agreement as it was.
//!
//! A third, smaller rule rides along: applying the SAME update twice leaves the same
//! record, since each named member replaces the stored one rather than accumulating.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees rather than what a handler returns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{assert_members, custom_directories, member_of, Answer, Emulator, ROLE};

/// `AgreementStatusType`'s whole modelled value set
/// (`smithy_query(query="shape",shape="AgreementStatusType")` reports
/// `[ACTIVE, INACTIVE]`).
const AGREEMENT_STATUSES: [&str; 2] = ["ACTIVE", "INACTIVE"];

/// A second `Role` satisfying `Role`'s modelled `len 20..2048`, pattern
/// `^arn:.*role/\S+$`, so a row that replaces `AccessRole` replaces it with another
/// value.
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-as2-scope";

/// Syntactically valid identifiers (`^a-([0-9a-f]{17})$`, `^p-([0-9a-f]{17})$`) that no
/// create ever minted, used by the refusal-order rows.
const NEVER_MINTED_AGREEMENT: &str = "a-0000000000000dead";
const NEVER_MINTED_PROFILE: &str = "p-0000000000000dead";

/// `base` with every member of `overrides` inserted over it.
fn with(base: &Value, overrides: Value) -> Value {
    let mut merged = base.clone();
    let object = merged.as_object_mut().expect("the base is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        object.insert(member.clone(), value.clone());
    }
    merged
}

/// One stored agreement, addressed the way the model addresses it: by the pair that makes
/// up `AgreementResource`'s identifiers.
#[derive(Clone, Debug)]
struct Stored {
    /// What this record is, named in every assertion about it.
    name: &'static str,
    server_id: String,
    agreement_id: String,
}

/// A `Description` the create can carry: `Description` is modelled `len 1..200`, pattern
/// `^[\u0021-\u007E]+$`, which admits no space, so each record's prose name cannot be its
/// description.
fn description_of(slug: &str) -> String {
    format!("as2-agreement-{slug}")
}

/// The fleet every test here writes against: two servers, the two profiles their
/// agreements combine, two more profiles an update can move an agreement to, and four
/// agreements spread over the two servers with different directory configurations.
struct Fleet {
    first_server: String,
    second_server: String,
    local_profile: String,
    partner_profile: String,
    other_local_profile: String,
    other_partner_profile: String,
    agreements: Vec<Stored>,
}

impl Fleet {
    async fn create(emulator: &Emulator) -> Self {
        let first_server = emulator.create_server(json!({})).await;
        let second_server = emulator.create_server(json!({})).await;
        let local_profile = emulator
            .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
            .await;
        let partner_profile = emulator
            .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
            .await;
        let other_local_profile = emulator
            .create_profile(json!({"As2Id": "local-party-2", "ProfileType": "LOCAL"}))
            .await;
        let other_partner_profile = emulator
            .create_profile(json!({"As2Id": "partner-party-2", "ProfileType": "PARTNER"}))
            .await;

        let mut fleet = Self {
            first_server,
            second_server,
            local_profile,
            partner_profile,
            other_local_profile,
            other_partner_profile,
            agreements: Vec::new(),
        };

        // Four records: two of the first server carrying a BaseDirectory, one carrying
        // CustomDirectories, and one of the SECOND server — so a write aimed at any one
        // of them has neighbours on its own server and on another.
        let plan: [(&'static str, &'static str, bool, Value); 4] = [
            (
                "the first agreement of the first server, on a base directory",
                "first",
                true,
                json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/first"}),
            ),
            (
                "the second agreement of the first server, on a base directory",
                "second",
                true,
                json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/second"}),
            ),
            (
                "the third agreement of the first server, on custom directories",
                "third",
                true,
                json!({"CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/third")}),
            ),
            (
                "the only agreement of the second server, on a base directory",
                "other-server",
                false,
                json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/other-server"}),
            ),
        ];
        for (name, slug, on_first_server, directories) in plan {
            let server_id = if on_first_server {
                fleet.first_server.clone()
            } else {
                fleet.second_server.clone()
            };
            let request = with(
                &json!({
                    "ServerId": &server_id,
                    "LocalProfileId": &fleet.local_profile,
                    "PartnerProfileId": &fleet.partner_profile,
                    "AccessRole": ROLE,
                    "Description": description_of(slug),
                }),
                directories,
            );
            let agreement_id = emulator.create_agreement(name, request).await;
            fleet.agreements.push(Stored {
                name,
                server_id,
                agreement_id,
            });
        }
        fleet
    }

    /// Every stored agreement as `DescribeAgreement` reports it right now, keyed by the
    /// pair that addresses it — the only reader this suite trusts for the state, so what
    /// is compared is what a client would see.
    async fn snapshot(&self, emulator: &Emulator, case: &str) -> BTreeMap<String, Value> {
        let mut records = BTreeMap::new();
        for stored in &self.agreements {
            let answer = emulator
                .describe_agreement(&stored.server_id, &stored.agreement_id)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected DescribeAgreement 200 for {}, got {} with body {}",
                stored.name, answer.status, answer.body
            );
            records.insert(
                format!("{}/{}", stored.server_id, stored.agreement_id),
                answer.get("Agreement").clone(),
            );
        }
        records
    }
}

/// Send one `UpdateAgreement` over the wire.
async fn update(emulator: &Emulator, server_id: &str, agreement_id: &str, members: &Value) -> Answer {
    emulator
        .call(
            "UpdateAgreement",
            with(
                members,
                json!({"ServerId": server_id, "AgreementId": agreement_id}),
            ),
        )
        .await
}

/// Assert that every record of `before` except `addressed` is still exactly as it was.
fn assert_untouched(
    case: &str,
    addressed: Option<&str>,
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) {
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "case {case:?}: expected the same set of readable agreements, got {:?}",
        after.keys().collect::<Vec<_>>()
    );
    for (key, stored) in before {
        if Some(key.as_str()) == addressed {
            continue;
        }
        let now = after.get(key).unwrap_or(&Value::Null);
        assert_eq!(
            now, stored,
            "case {case:?}: expected the agreement {key} to be untouched, got {now:?} \
             instead of {stored:?}"
        );
    }
}

/// RULE 1: an update writes exactly the record it addresses — the addressed agreement
/// carries the new values, and every other stored agreement is untouched.
///
/// The rows differ along which record of the fleet is addressed (each of the four in
/// turn, over both servers), the directory configuration that record starts with, and
/// which members the update names (status, description, role, both profiles, a base
/// directory, custom directories replacing one, and several at once).
#[tokio::test]
async fn an_update_writes_only_the_agreement_it_addresses() {
    let emulator = Emulator::new();
    let fleet = Fleet::create(&emulator).await;

    // (which record of the fleet, the members the update names, the members the addressed
    // record must then report)
    let rows: Vec<(usize, Value, Value)> = vec![
        (
            0,
            json!({"Status": AGREEMENT_STATUSES[1]}),
            json!({"Status": AGREEMENT_STATUSES[1]}),
        ),
        (
            1,
            json!({"Description": "re-described", "AccessRole": OTHER_ROLE}),
            json!({"Description": "re-described", "AccessRole": OTHER_ROLE}),
        ),
        (
            2,
            json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/moved-off-custom"}),
            json!({
                "BaseDirectory": "/amzn-s3-demo-bucket/home/moved-off-custom",
                "CustomDirectories": null,
            }),
        ),
        (
            0,
            json!({"CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/moved")}),
            json!({
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/moved"),
                "BaseDirectory": null,
            }),
        ),
        (
            3,
            json!({
                "LocalProfileId": &fleet.other_local_profile,
                "PartnerProfileId": &fleet.other_partner_profile,
                "Status": AGREEMENT_STATUSES[1],
                "PreserveFilename": "ENABLED",
                "EnforceMessageSigning": "ENABLED",
            }),
            json!({
                "LocalProfileId": &fleet.other_local_profile,
                "PartnerProfileId": &fleet.other_partner_profile,
                "Status": AGREEMENT_STATUSES[1],
                "PreserveFilename": "ENABLED",
                "EnforceMessageSigning": "ENABLED",
            }),
        ),
    ];

    for (index, members, expected) in rows {
        let stored = fleet.agreements[index].clone();
        let case = format!("{} updated with {}", stored.name, members);
        let case = case.as_str();
        let key = format!("{}/{}", stored.server_id, stored.agreement_id);

        let before = fleet.snapshot(&emulator, case).await;
        let answer = update(&emulator, &stored.server_id, &stored.agreement_id, &members).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateAgreement 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("AgreementId").as_str(),
            Some(stored.agreement_id.as_str()),
            "case {case:?}: expected the addressed AgreementId back, got {}",
            answer.body
        );

        let after = fleet.snapshot(&emulator, case).await;
        let addressed = after
            .get(&key)
            .unwrap_or_else(|| panic!("case {case:?}: the addressed agreement reads back"));
        assert_members(case, addressed, &expected);
        // The identity of the record never moves with its configuration.
        assert_members(
            case,
            addressed,
            &json!({
                "AgreementId": &stored.agreement_id,
                "ServerId": &stored.server_id,
                "Arn": member_of(&before[&key], "Arn").clone(),
            }),
        );
        assert_untouched(case, Some(key.as_str()), &before, &after);
    }
}

/// RULE 2: a request that breaks several rules at once is answered for the rule the
/// service decides first, and a refused update writes nothing — every stored agreement,
/// the addressed one included, reads back as it stood.
///
/// The rows differ along which rules the request breaks together, which half of the
/// addressed pair misses, and which profile member carries the unresolvable reference.
#[tokio::test]
async fn a_request_that_breaks_several_rules_is_answered_for_the_first_one() {
    let emulator = Emulator::new();
    let fleet = Fleet::create(&emulator).await;
    let stored = fleet.agreements[0].clone();

    // Both directory configurations at once, which the documented either/or forbids
    // ("Specify either BaseDirectory or CustomDirectories, but not both").
    let both_directories = json!({
        "BaseDirectory": "/amzn-s3-demo-bucket/home/both",
        "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/both"),
    });
    // A change every refused row carries, so a row that was silently applied shows up in
    // the read that follows rather than passing unnoticed.
    let visible = json!({"Description": "must-not-land", "Status": AGREEMENT_STATUSES[1]});

    // (case, the addressed pair, the members the request names, the expected error code)
    let rows: Vec<(&str, String, String, Value, &str)> = vec![
        (
            "both directory configurations on an agreement that exists",
            stored.server_id.clone(),
            stored.agreement_id.clone(),
            with(&visible, both_directories.clone()),
            "InvalidRequestException",
        ),
        (
            "both directory configurations on an AgreementId nothing minted: the \
             either/or is decided before the lookup",
            stored.server_id.clone(),
            NEVER_MINTED_AGREEMENT.to_string(),
            with(&visible, both_directories.clone()),
            "InvalidRequestException",
        ),
        (
            "both directory configurations under a ServerId that holds no such agreement",
            fleet.second_server.clone(),
            stored.agreement_id.clone(),
            with(&visible, both_directories.clone()),
            "InvalidRequestException",
        ),
        (
            "both directory configurations beside an unresolvable LocalProfileId",
            stored.server_id.clone(),
            stored.agreement_id.clone(),
            with(
                &with(&visible, both_directories.clone()),
                json!({"LocalProfileId": NEVER_MINTED_PROFILE}),
            ),
            "InvalidRequestException",
        ),
        (
            "an unresolvable PartnerProfileId on an AgreementId nothing minted: the \
             addressed pair is resolved before the reference",
            stored.server_id.clone(),
            NEVER_MINTED_AGREEMENT.to_string(),
            with(&visible, json!({"PartnerProfileId": NEVER_MINTED_PROFILE})),
            "ResourceNotFoundException",
        ),
        (
            "an unresolvable LocalProfileId on an agreement that exists",
            stored.server_id.clone(),
            stored.agreement_id.clone(),
            with(&visible, json!({"LocalProfileId": NEVER_MINTED_PROFILE})),
            "ResourceNotFoundException",
        ),
    ];

    for (case, server_id, agreement_id, members, code) in rows {
        let before = fleet.snapshot(&emulator, case).await;
        let answer = update(&emulator, &server_id, &agreement_id, &members).await;
        // 400 is `InvalidRequestException`'s modelled `@httpError`, and the status this
        // service's recorded wire answers `ResourceNotFoundException` with.
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, code),
            "case {case:?}: expected HTTP 400 {code}, got {} with body {}",
            answer.status,
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty()
                && !message.contains(&agreement_id)
                && !message.contains(NEVER_MINTED_PROFILE),
            "case {case:?}: expected a non-empty Message carrying no request identifier, \
             got {message:?}"
        );

        let after = fleet.snapshot(&emulator, case).await;
        assert_untouched(case, None, &before, &after);
    }
}

/// RULE 3: the same update applied twice leaves the same record — each member the request
/// names REPLACES the stored one, so a repeated write is a no-op rather than an
/// accumulation.
#[tokio::test]
async fn the_same_update_applied_twice_leaves_the_same_record() {
    let emulator = Emulator::new();
    let fleet = Fleet::create(&emulator).await;

    // One row per shape of update: a status change, a directory configuration replacing
    // the other, and several members at once.
    let rows: Vec<(&str, usize, Value)> = vec![
        (
            "a repeated status change",
            0,
            json!({"Status": AGREEMENT_STATUSES[1]}),
        ),
        (
            "a repeated move from a base directory to custom directories",
            1,
            json!({"CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/twice")}),
        ),
        (
            "a repeated move from custom directories to a base directory, with a \
             description",
            2,
            json!({
                "BaseDirectory": "/amzn-s3-demo-bucket/home/twice",
                "Description": "written-twice",
            }),
        ),
    ];

    for (case, index, members) in rows {
        let stored = fleet.agreements[index].clone();
        for attempt in ["first", "second"] {
            let answer = update(&emulator, &stored.server_id, &stored.agreement_id, &members).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({attempt} attempt): expected UpdateAgreement 200, got {} \
                 with body {}",
                answer.status, answer.body
            );
        }
        let once = emulator
            .describe_agreement(&stored.server_id, &stored.agreement_id)
            .await;
        assert_eq!(
            once.status, 200,
            "case {case:?}: expected DescribeAgreement 200, got {} with body {}",
            once.status, once.body
        );
        let after_two = once.get("Agreement").clone();

        // A third write of the same request must still leave the same record.
        let answer = update(&emulator, &stored.server_id, &stored.agreement_id, &members).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} (third attempt): expected UpdateAgreement 200, got {} with \
             body {}",
            answer.status, answer.body
        );
        let after_three = emulator
            .describe_agreement(&stored.server_id, &stored.agreement_id)
            .await
            .get("Agreement")
            .clone();
        assert_eq!(
            after_three, after_two,
            "case {case:?}: expected a repeated update to leave the same record, got \
             {after_three:?} instead of {after_two:?}"
        );
    }
}
