// SPDX-License-Identifier: CC0-1.0

//! How every operation that acts on ONE named volume reads the `VolumeId` it was given,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the SPLIT between the two refusal classes: EC2 validates a volume
//! id against the form it ISSUES before it looks anything up, so a value outside that form is
//! `InvalidVolumeID.Malformed` (the id-level class, worded `The volume ID '<the value sent>'
//! is malformed` — the sentence AWS words this id kind's unreadable values with), and only a
//! value INSIDE it that names no volume of the account is `InvalidVolume.NotFound`. The two
//! mean different things to a caller: the first is a permanent input bug, the second a
//! resource that may yet appear — so a client that retries on not-found spins forever on an
//! id EC2 could never have issued.
//!
//! The issued form is the two forms EC2 has published — 8 or 17 lowercase hexadecimal digits
//! — with the long one led by `0` (it carries 68 digits' worth of a 64-bit identifier) and
//! never the all-zero placeholder no mint can reach
//! (`ec2::state::volume_id_is_well_formed`). The table below therefore spans that class
//! rather than one recorded id: each way a value can fall outside the form (a leading digit
//! above zero, every digit above zero, the placeholder, a non-hexadecimal digit, a digit
//! count between the two forms, another resource kind's prefix, upper case) beside each way
//! it can fall inside it and still name nothing (both published forms) — and the volume the
//! account really holds, so every row is about the id it names rather than about an empty
//! store.
//!
//! Every row is asked of SEVERAL operations, because the reading belongs to the service and
//! not to one handler: `ModifyVolumeAttribute`, `DescribeVolumeAttribute` and
//! `EnableVolumeIO` each act on the one volume their request names.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The status EC2 answers both refusal classes with.
const REFUSAL: u16 = 400;

/// The id-level class for a value EC2 cannot read as a volume id at all.
const MALFORMED: &str = "InvalidVolumeID.Malformed";

/// The class for an id inside the issued form that names no volume of the account.
const NOT_FOUND: &str = "InvalidVolume.NotFound";

/// Which refusal class a row is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Malformed,
    NotFound,
    /// The volume the account holds: no refusal at all.
    Served,
}

impl Class {
    fn code(self) -> &'static str {
        match self {
            Self::Malformed => MALFORMED,
            Self::NotFound => NOT_FOUND,
            Self::Served => "",
        }
    }
}

/// One row: what the value is, the value itself, and the class it is due.
struct Row {
    case: &'static str,
    volume_id: String,
    class: Class,
}

/// How one operation names the volume its request is about, given the id under test.
type VolumeParams = fn(&str) -> Vec<Param>;

fn p(name: &str, value: &str) -> Param {
    param(name, value)
}

/// The zone the volume under test is created in: the Region's first, read off
/// `DescribeAvailabilityZones` rather than written here.
async fn first_zone(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, REGION, "DescribeAvailabilityZones", &[]).await;
    assert_eq!(
        status, 200,
        "the Region must report its zones, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "zoneName")
        .unwrap_or_else(|| panic!("DescribeAvailabilityZones must report a zone: {body}"))
}

/// One `CreateVolume`, as the id it minted — the one id in the table that resolves.
async fn create_volume(router: &MultiServiceRouter) -> String {
    let zone = first_zone(router).await;
    let (status, body) = call(
        router,
        REGION,
        "CreateVolume",
        &[
            p("AvailabilityZone", &zone),
            p("VolumeType", "gp3"),
            p("Size", "8"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the volume under test must be created, got {status} with body {body}"
    );
    child_text(document("the created volume", &body).root_element(), "volumeId")
        .unwrap_or_else(|| panic!("CreateVolume must report the volume it minted: {body}"))
}

/// Every row of the class table, given the one volume the account holds.
fn rows(held: &str) -> Vec<Row> {
    let owned = |case: &'static str, volume_id: String, class: Class| Row {
        case,
        volume_id,
        class,
    };
    vec![
        owned(
            "a long-form id whose leading digit is above zero, which the 64-bit identifier \
             never carries",
            "vol-1234567890abcdef0".to_string(),
            Class::Malformed,
        ),
        owned(
            "a long-form id with every digit above zero",
            "vol-fffffffffffffffff".to_string(),
            Class::Malformed,
        ),
        owned(
            "the all-zero placeholder no mint can issue",
            format!("vol-{}", "0".repeat(17)),
            Class::Malformed,
        ),
        owned(
            "a digit that is not hexadecimal",
            "vol-0z23456789abcdef0".to_string(),
            Class::Malformed,
        ),
        owned(
            "a digit count between the two published forms",
            "vol-0123456789".to_string(),
            Class::Malformed,
        ),
        owned(
            "one digit past the long form",
            format!("vol-{}", "0".repeat(18)),
            Class::Malformed,
        ),
        owned(
            "the long form in upper case",
            "vol-0123456789ABCDEF0".to_string(),
            Class::Malformed,
        ),
        owned(
            "another resource kind's id",
            format!("{}-00000000000000001", ec2::state::SNAPSHOT_ID_PREFIX),
            Class::Malformed,
        ),
        owned(
            "no prefix at all",
            "0123456789abcdef0".to_string(),
            Class::Malformed,
        ),
        owned(
            "an id inside the issued long form nothing was created for",
            "vol-00000000000000999".to_string(),
            Class::NotFound,
        ),
        owned(
            "an id inside the retired short form nothing was created for",
            "vol-0abc1234".to_string(),
            Class::NotFound,
        ),
        owned(
            "the volume the account holds",
            held.to_string(),
            Class::Served,
        ),
    ]
}

/// RULE: `ModifyVolumeAttribute` reads its `VolumeId` against the form EC2 ISSUES before any
/// lookup — a value outside it is `InvalidVolumeID.Malformed` with the id-level sentence
/// naming the value as sent, a value inside it naming nothing is `InvalidVolume.NotFound`,
/// and the volume the account holds is modified.
#[tokio::test]
async fn modify_volume_attribute_splits_the_two_volume_id_refusal_classes() {
    let (router, _engine) = super::ec2_query::emulator();
    let held = create_volume(&router).await;

    for row in rows(&held) {
        let case = row.case;
        let (status, body) = call(
            &router,
            REGION,
            "ModifyVolumeAttribute",
            &[
                p("VolumeId", &row.volume_id),
                p("AutoEnableIO.Value", "true"),
            ],
        )
        .await;
        if row.class == Class::Served {
            assert_eq!(
                status, 200,
                "case {case:?} ({}): the volume the account holds must be modified, got \
                 {status} with body {body}",
                row.volume_id
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (REFUSAL, row.class.code()),
            "case {case:?} ({}): expected {REFUSAL} {}, got {status} with body {body}",
            row.volume_id,
            row.class.code()
        );
        if row.class == Class::Malformed {
            assert_eq!(
                error_message(&body),
                format!("The volume ID '{}' is malformed", row.volume_id),
                "case {case:?}: expected the id-level sentence naming the value as sent, got \
                 body {body}"
            );
        }
    }

    // The modification the served row made is the one the volume now reports, so the table
    // above refused the ids it named rather than the operation.
    let (status, body) = call(
        &router,
        REGION,
        "DescribeVolumeAttribute",
        &[
            p("VolumeId", &held),
            p("Attribute", ec2::state::VOLUME_ATTRIBUTE_AUTO_ENABLE_IO),
        ],
    )
    .await;
    assert_eq!(
        (status, super::ec2_query::element(&body, "value").as_deref()),
        (200, Some("true")),
        "the volume the account holds must report the attribute the served row set, got \
         {status} with body {body}"
    );
}

/// The SAME split for the other operations that act on one named volume: the reading belongs
/// to the service, so an id class cannot be answered one way by one handler and another way
/// by the next. Only the REFUSAL rows are asked here — what each of these operations does
/// with a volume it resolves is its own suite's business.
#[tokio::test]
async fn every_operation_on_one_named_volume_splits_the_same_two_classes() {
    let (router, _engine) = super::ec2_query::emulator();
    let held = create_volume(&router).await;

    // (the operation, how it names the volume — a scalar `VolumeId` member for the three
    // that act on ONE volume, an indexed `VolumeId.N` list for the describe that takes
    // several)
    let operations: [(&str, VolumeParams); 4] = [
        ("DescribeVolumeAttribute", |volume_id| {
            vec![
                param("VolumeId", volume_id),
                param("Attribute", ec2::state::VOLUME_ATTRIBUTE_AUTO_ENABLE_IO),
            ]
        }),
        ("EnableVolumeIO", |volume_id| {
            vec![param("VolumeId", volume_id)]
        }),
        ("DescribeVolumesModifications", |volume_id| {
            super::ec2_query::indexed("VolumeId", &[volume_id])
        }),
        ("DeleteVolume", |volume_id| {
            vec![param("VolumeId", volume_id)]
        }),
    ];

    for (action, params_for) in &operations {
        for row in rows(&held) {
            let case = row.case;
            if row.class == Class::Served {
                continue;
            }
            let params = params_for(&row.volume_id);
            let (status, body) = call(&router, REGION, action, &params).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (REFUSAL, row.class.code()),
                "case {case:?} ({action} of {}): expected {REFUSAL} {}, got {status} with \
                 body {body}",
                row.volume_id,
                row.class.code()
            );
        }
    }

    // None of those refusals touched the volume the account holds, so each was about the id
    // the request named.
    let (status, body) = call(
        &router,
        REGION,
        "DescribeVolumes",
        &super::ec2_query::indexed("VolumeId", &[held.as_str()]),
    )
    .await;
    assert_eq!(
        (status, super::ec2_query::element(&body, "volumeId").as_deref()),
        (200, Some(held.as_str())),
        "the volume the account holds must still be describable, got {status} with body \
         {body}"
    );
}
