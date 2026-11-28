// SPDX-License-Identifier: CC0-1.0

//! WHICH calls charge the BYTES of data transformation profile content this
//! emulator retains.
//!
//! The rule under test: every operation that adds retained profile version content
//! — the mint, the DRAFT rewrite and the publish — charges the ceiling on what ONE
//! profile retains and the ceiling on what EVERY profile retains together, taken
//! over the store as it stands less the bytes the same write gives back, BEFORE it
//! writes anything, and refuses past either with the capacity error it models. So
//! no operation can push the store past a ceiling another operation is then
//! refused for, and a refused call retains nothing.
//!
//! The rows differ along what the rule generalises over: WHICH call adds the
//! content (create, update, publish), how many bytes it adds beside how many it
//! releases (more, exactly as many, fewer), and WHICH of the two ceilings the
//! bytes reach.
//!
//! The content the ceilings are charged against is seeded onto the holder records
//! through the shared engine the harness exposes — the way
//! `tests/all/start_data_transformation_job.rs` seeds a profile's published
//! versions — rather than written as tens of megabytes of create bodies; every
//! seeded version holds at most the bytes one mapping may carry, so no holder is a
//! record an operation could not have written. The CHARGED call itself always goes
//! over the real awsJson1_0 wire through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter), and
//! so does every read-back.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its shape's
//! `@httpError` names, for the reason `tests/all/error_envelopes.rs` pins: awsJson
//! binds nothing to the transport, so this service reports an error's FAULT.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    mapping_bytes, profile_arn, ComAmazonawsHealthlakeHealthlakeState, DataTransformationProfile,
    ProfileVersionRecord, DRAFT_PROFILE_VERSION, MAX_PROFILES, MAX_PROFILE_MAPPING_BYTES,
    MAX_PROFILE_RETAINED_BYTES, MAX_PROFILE_RETAINED_BYTES_TOTAL,
};

use crate::data_transformation_profile::{
    mapping_of_exactly, profile_mapping_source, reported_mapping,
};
use crate::fhir_datastore_lifecycle::{error_code, error_message, string_at, HealthLake};
use crate::publish_data_transformation_profile::request_mapping;

/// The bytes a seeded version holds at most: what one mapping may carry, so the
/// seeded content is content a create or an update would have admitted.
const VERSION_CHUNK: usize = MAX_PROFILE_MAPPING_BYTES;

/// What the subject profile's DRAFT holds: small beside both ceilings, so the
/// charged call is the one whose bytes reach them.
const SUBJECT_BYTES: usize = 4_096;

/// The content a charging create or update carries: the most one mapping may hold,
/// so a store already at its ceiling has to refuse it.
const CHARGED_BYTES: usize = MAX_PROFILE_MAPPING_BYTES;

/// The region a seeded holder's ARN is built for. Only the record's content is
/// charged, so the region is any one this service serves.
const SEED_REGION: &str = "us-east-1";

/// The source format every profile in this suite converts from, one of the
/// modelled `SourceFormat` values (CCDA | CSV).
///
/// The TEMPLATE format, because the contents below are sized to sit on a byte
/// ceiling rather than shaped to a format: a publish naming the
/// comma-separated-values format admits one mapping file only, which is a rule
/// about shape that `tests/all/publish_data_transformation_profile.rs` pins.
const SOURCE_FORMAT: &str = "CCDA";

/// The only modelled `TargetFormat` value, which a create documents as "Always
/// FHIR_R4".
const TARGET_FORMAT: &str = "FHIR_R4";

/// The status this service answers a served call with: awsJson1_0 binds nothing to
/// the transport, so every operation succeeds with 200 — the create's modelled
/// `@http(code: 201)` is inert on the wire.
const SERVED_STATUS: u16 = 200;

/// A mapping whose keys and values hold EXACTLY `bytes` bytes together, from the
/// shared profile harness, with the size this suite's rows depend on re-asserted
/// here: a row that must sit ON a ceiling is only a test of the ceiling if its
/// content is exactly that size.
fn sized_mapping(bytes: usize) -> BTreeMap<String, String> {
    let content = mapping_of_exactly(bytes);
    assert_eq!(
        mapping_bytes(&content),
        bytes,
        "the built mapping must hold exactly {bytes} bytes"
    );
    content
}

/// How many bytes of profile content this service retains, as the store itself
/// measures it.
fn retained(service: &HealthLake) -> usize {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    // Bound rather than returned straight: the guard is a temporary of the final
    // expression, so it would outlive `store` itself.
    let retained = store.lock().profile_retained_bytes_total();
    retained
}

/// Create a profile over the wire whose DRAFT holds exactly `bytes` bytes, and
/// report the `ProfileId` it minted.
async fn create_subject(service: &HealthLake, case: &str, bytes: usize) -> String {
    let (status, created) = service
        .call(
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "ceiling.subject",
                "SourceFormat": SOURCE_FORMAT,
                "Source": profile_mapping_source(&sized_mapping(bytes)),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the subject create must answer the protocol's 200, got {status}: {created}"
    );
    string_at(&created, "ProfileId")
}

/// Seed held profiles until this service retains exactly `total` bytes of profile
/// content, each holder carrying a DRAFT of at most [`VERSION_CHUNK`] bytes.
fn fill_retained_bytes(service: &HealthLake, case: &str, total: usize) {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    let mut held = store.profile_retained_bytes_total();
    assert!(
        held <= total,
        "case {case:?}: the store already retains {held} bytes, past the {total} this row fills \
         to"
    );
    while held < total {
        let chunk = (total - held).min(VERSION_CHUNK);
        let now = SystemTime::now();
        let created_at = ::aws_smithy_types::DateTime::from(now);
        let id = store.next_profile_id(now);
        let holder = DataTransformationProfile {
            arn: profile_arn(SEED_REGION, &id),
            id: id.clone(),
            name: format!("ceiling.holder-{id}"),
            description: None,
            source_format: SOURCE_FORMAT.to_string(),
            target_format: TARGET_FORMAT.to_string(),
            kms_key_id: None,
            created_at,
            draft: ProfileVersionRecord {
                version: DRAFT_PROFILE_VERSION,
                mapping: sized_mapping(chunk),
                change_description: None,
                last_updated_at: created_at,
            },
            published: BTreeMap::new(),
            tags: BTreeMap::new(),
        };
        store.profiles.insert(id, holder);
        held += chunk;
    }
    assert_eq!(
        store.profile_retained_bytes_total(),
        total,
        "case {case:?}: the seeded holders must bring what the store retains to {total}"
    );
    assert!(
        store.profiles.len() <= MAX_PROFILES,
        "case {case:?}: the holders must fit inside the profile count ceiling, got {}",
        store.profiles.len()
    );
}

/// Seed `total` bytes of PUBLISHED versions onto the profile `profile_id` names, in
/// versions of at most [`VERSION_CHUNK`] bytes numbered up from the DRAFT.
fn seed_published_bytes(service: &HealthLake, case: &str, profile_id: &str, total: usize) {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    let profile = store
        .profiles
        .get_mut(profile_id)
        .unwrap_or_else(|| panic!("case {case:?}: the created profile {profile_id:?} must be held"));
    let mut seeded = 0usize;
    while seeded < total {
        let chunk = (total - seeded).min(VERSION_CHUNK);
        let version = profile
            .published
            .keys()
            .next_back()
            .copied()
            .unwrap_or(DRAFT_PROFILE_VERSION)
            + 1;
        profile.published.insert(
            version,
            ProfileVersionRecord {
                version,
                mapping: sized_mapping(chunk),
                change_description: None,
                last_updated_at: profile.created_at,
            },
        );
        seeded += chunk;
    }
}

/// The `GetDataTransformationProfile` body for one version of a profile.
async fn get_version(service: &HealthLake, profile_id: &str, version: i32) -> (u16, Value) {
    service
        .call(
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": version }),
        )
        .await
}

/// Which call adds the retained content a row charges the ceilings with.
#[derive(Clone, Copy, Debug)]
enum Charge {
    /// `CreateDataTransformationProfile`: the minted profile retains its DRAFT.
    Create,
    /// `UpdateDataTransformationProfile`: the rewritten DRAFT retains the new
    /// content and gives the content it replaced back.
    Update,
    /// `PublishDataTransformationProfile`: the snapshot is retained beside every
    /// version the profile already holds.
    Publish,
}

impl Charge {
    /// The operation this charge calls.
    fn operation(self) -> &'static str {
        match self {
            Self::Create => "CreateDataTransformationProfile",
            Self::Update => "UpdateDataTransformationProfile",
            Self::Publish => "PublishDataTransformationProfile",
        }
    }

    /// The modelled error this operation refuses a capacity it cannot retain with:
    /// the quota-shaped error where it declares one, and the throttling-shaped one
    /// where it does not — the same choice the sibling writes that charge a
    /// retention ceiling make.
    fn refusal_code(self) -> &'static str {
        match self {
            Self::Create | Self::Publish => "ServiceQuotaExceededException",
            Self::Update => "ThrottlingException",
        }
    }

    /// The request body this charge sends against the subject profile.
    fn request(self, subject: &str, content: &BTreeMap<String, String>) -> Value {
        match self {
            Self::Create => json!({
                "ProfileName": "ceiling.minted",
                "SourceFormat": SOURCE_FORMAT,
                "Source": profile_mapping_source(content),
            }),
            Self::Update => json!({
                "ProfileId": subject,
                "ProfileMapping": request_mapping(content),
            }),
            Self::Publish => json!({
                "ProfileId": subject,
                "SourceFormat": SOURCE_FORMAT,
            }),
        }
    }
}

/// How full the store is when a row's charging call arrives.
#[derive(Clone, Copy, Debug)]
enum Fill {
    /// Exactly at the store-wide ceiling: every further byte is one the service
    /// will not retain.
    AtTheCeiling,
    /// A quarter of it, so the charged content fits with room to spare.
    WellInside,
}

#[tokio::test]
async fn every_write_that_retains_profile_content_charges_the_store_wide_ceiling() {
    for charge in [Charge::Create, Charge::Update, Charge::Publish] {
        for fill in [Fill::AtTheCeiling, Fill::WellInside] {
            let case = format!("{charge:?} with the store {fill:?}");
            let service = HealthLake::new();
            let subject = create_subject(&service, &case, SUBJECT_BYTES).await;
            let draft = sized_mapping(SUBJECT_BYTES);
            let content = sized_mapping(CHARGED_BYTES);
            let filled = match fill {
                Fill::AtTheCeiling => MAX_PROFILE_RETAINED_BYTES_TOTAL,
                Fill::WellInside => MAX_PROFILE_RETAINED_BYTES_TOTAL / 4,
            };
            fill_retained_bytes(&service, &case, filled);

            let (status, payload) = service
                .call(charge.operation(), &charge.request(&subject, &content))
                .await;

            match fill {
                Fill::AtTheCeiling => {
                    assert_eq!(
                        (status, error_code(&payload).as_str()),
                        (wire_http_status(400), charge.refusal_code()),
                        "case {case:?}: a call the store cannot retain is refused with its \
                         modelled capacity error, got {status}: {payload}"
                    );
                    let message = error_message(&payload);
                    assert!(
                        message.contains(&MAX_PROFILE_RETAINED_BYTES_TOTAL.to_string()),
                        "case {case:?}: the refusal must name the store-wide ceiling \
                         {MAX_PROFILE_RETAINED_BYTES_TOTAL}, got {message:?}"
                    );
                    assert_eq!(
                        retained(&service),
                        filled,
                        "case {case:?}: a refused call must retain nothing"
                    );
                    // The subject is exactly as it was: the DRAFT still carries the
                    // content it was created with and no version was published.
                    let (status, body) = get_version(&service, &subject, DRAFT_PROFILE_VERSION)
                        .await;
                    assert_eq!(
                        status,
                        200,
                        "case {case:?}: the subject's DRAFT must still read back, got {status}: \
                         {body}"
                    );
                    assert_eq!(
                        reported_mapping(&case, &body),
                        draft,
                        "case {case:?}: a refused call must leave the stored DRAFT where it was"
                    );
                    let (status, body) = get_version(&service, &subject, 1).await;
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (wire_http_status(400), "ResourceNotFoundException"),
                        "case {case:?}: a refused call must publish no version, got {status}: \
                         {body}"
                    );
                }
                Fill::WellInside => {
                    assert_eq!(
                        status, SERVED_STATUS,
                        "case {case:?}: a call the store can retain must be served, got {status}: \
                         {payload}"
                    );
                    // What was served is readable back through the modelled reader,
                    // carrying exactly the content the call retained — and the store
                    // retains the bytes that call added, less the ones it released.
                    let (expected_id, expected_version, expected_content, expected_total) =
                        match charge {
                            Charge::Create => (
                                string_at(&payload, "ProfileId"),
                                DRAFT_PROFILE_VERSION,
                                content.clone(),
                                filled + CHARGED_BYTES,
                            ),
                            Charge::Update => (
                                subject.clone(),
                                DRAFT_PROFILE_VERSION,
                                content.clone(),
                                filled - SUBJECT_BYTES + CHARGED_BYTES,
                            ),
                            Charge::Publish => (
                                subject.clone(),
                                1,
                                draft.clone(),
                                filled + SUBJECT_BYTES,
                            ),
                        };
                    let (status, body) =
                        get_version(&service, &expected_id, expected_version).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: what the served call retained must read back, got \
                         {status}: {body}"
                    );
                    assert_eq!(
                        reported_mapping(&case, &body),
                        expected_content,
                        "case {case:?}: the stored content must be what the served call carried"
                    );
                    assert_eq!(
                        retained(&service),
                        expected_total,
                        "case {case:?}: a served call retains exactly the bytes it added, less \
                         the ones it released"
                    );
                }
            }
        }
    }
}

/// One row of the "charged for what it ADDS" table: the bytes the DRAFT holds
/// before the update, the bytes the update carries, and whether a store already at
/// its store-wide ceiling admits it.
struct AddedRow {
    case: &'static str,
    draft_bytes: usize,
    update_bytes: usize,
    admitted: bool,
}

#[tokio::test]
async fn an_update_at_the_ceiling_is_charged_only_for_the_bytes_it_adds() {
    // The DRAFT an update REPLACES gives its bytes back, so a store at its ceiling
    // admits an update that carries no more than the DRAFT already holds and
    // refuses one that carries more. The rows run across that boundary.
    let rows = [
        AddedRow {
            case: "exactly the bytes the DRAFT already holds",
            draft_bytes: 524_288,
            update_bytes: 524_288,
            admitted: true,
        },
        AddedRow {
            case: "half the bytes the DRAFT holds",
            draft_bytes: 524_288,
            update_bytes: 262_144,
            admitted: true,
        },
        AddedRow {
            case: "one byte more than the DRAFT holds",
            draft_bytes: 524_288,
            update_bytes: 524_289,
            admitted: false,
        },
        AddedRow {
            case: "a whole mapping over a small DRAFT",
            draft_bytes: SUBJECT_BYTES,
            update_bytes: MAX_PROFILE_MAPPING_BYTES,
            admitted: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let service = HealthLake::new();
        let subject = create_subject(&service, case, row.draft_bytes).await;
        fill_retained_bytes(&service, case, MAX_PROFILE_RETAINED_BYTES_TOTAL);
        let content = sized_mapping(row.update_bytes);

        let (status, payload) = service
            .call(
                "UpdateDataTransformationProfile",
                &json!({
                    "ProfileId": &subject,
                    "ProfileMapping": request_mapping(&content),
                }),
            )
            .await;

        if row.admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: an update that adds no more than it releases must be served, got \
                 {status}: {payload}"
            );
            let (status, body) = get_version(&service, &subject, DRAFT_PROFILE_VERSION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the rewritten DRAFT must read back, got {status}: {body}"
            );
            assert_eq!(
                reported_mapping(case, &body),
                content,
                "case {case:?}: the DRAFT must carry exactly what the update wrote"
            );
            assert_eq!(
                retained(&service),
                MAX_PROFILE_RETAINED_BYTES_TOTAL - row.draft_bytes + row.update_bytes,
                "case {case:?}: the store retains the new content in place of the old"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (wire_http_status(400), "ThrottlingException"),
                "case {case:?}: an update that adds more than the store can retain is refused, \
                 got {status}: {payload}"
            );
            assert_eq!(
                retained(&service),
                MAX_PROFILE_RETAINED_BYTES_TOTAL,
                "case {case:?}: a refused update must retain nothing"
            );
            let (status, body) = get_version(&service, &subject, DRAFT_PROFILE_VERSION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the subject's DRAFT must still read back, got {status}: {body}"
            );
            assert_eq!(
                reported_mapping(case, &body),
                sized_mapping(row.draft_bytes),
                "case {case:?}: a refused update must leave the stored DRAFT where it was"
            );
        }
    }
}

#[tokio::test]
async fn an_update_charges_the_bytes_one_profile_retains() {
    // The room the subject's own ceiling leaves once it holds its seeded versions:
    // an update that fills it exactly is served, and one byte more is refused.
    // What the whole store retains stays far inside its own ceiling here, so the
    // per-profile ceiling is the one that answers.
    const ROOM: usize = 262_144;
    for (case, update_bytes, admitted) in [
        ("the update that fills the profile's last room", ROOM + SUBJECT_BYTES, true),
        (
            "one byte past the profile's last room",
            ROOM + SUBJECT_BYTES + 1,
            false,
        ),
    ] {
        let service = HealthLake::new();
        let subject = create_subject(&service, case, SUBJECT_BYTES).await;
        seed_published_bytes(
            &service,
            case,
            &subject,
            MAX_PROFILE_RETAINED_BYTES - ROOM - SUBJECT_BYTES,
        );
        assert!(
            retained(&service) < MAX_PROFILE_RETAINED_BYTES_TOTAL,
            "case {case:?}: the store-wide ceiling must not be what answers this row, the store \
             retains {}",
            retained(&service)
        );
        let content = sized_mapping(update_bytes);

        let (status, payload) = service
            .call(
                "UpdateDataTransformationProfile",
                &json!({
                    "ProfileId": &subject,
                    "ProfileMapping": request_mapping(&content),
                }),
            )
            .await;

        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: an update inside what one profile retains must be served, got \
                 {status}: {payload}"
            );
            let (status, body) = get_version(&service, &subject, DRAFT_PROFILE_VERSION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the rewritten DRAFT must read back, got {status}: {body}"
            );
            assert_eq!(
                reported_mapping(case, &body),
                content,
                "case {case:?}: the DRAFT must carry exactly what the update wrote"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (wire_http_status(400), "ThrottlingException"),
                "case {case:?}: an update past what one profile retains is refused, got {status}: \
                 {payload}"
            );
            let message = error_message(&payload);
            assert!(
                message.contains(&MAX_PROFILE_RETAINED_BYTES.to_string()),
                "case {case:?}: the refusal must name the per-profile ceiling \
                 {MAX_PROFILE_RETAINED_BYTES}, got {message:?}"
            );
            let (status, body) = get_version(&service, &subject, DRAFT_PROFILE_VERSION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the subject's DRAFT must still read back, got {status}: {body}"
            );
            assert_eq!(
                reported_mapping(case, &body),
                sized_mapping(SUBJECT_BYTES),
                "case {case:?}: a refused update must leave the stored DRAFT where it was"
            );
        }
    }
}
