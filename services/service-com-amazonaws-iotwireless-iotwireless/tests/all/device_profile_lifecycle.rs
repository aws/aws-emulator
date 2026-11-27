// SPDX-License-Identifier: CC0-1.0

//! The device-profile lifecycle, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * `CreateDeviceProfile` persists the profile it was given and mints its identity, so
//!   `GetDeviceProfile` reports back every member that was stored, for every technology
//!   the modelled `DeviceProfileType` catalog names;
//! * a minted `Id` is a fresh uuid and its ARN carries the region the request names;
//! * an `@idempotencyToken` replay with the same parameters returns the first call's
//!   resource, and with different parameters is a 409;
//! * a request a modelled constraint or a documented name rule refuses persists
//!   nothing;
//! * an `Id` the store does not hold is a 404.
//!
//! The wire harness is the one the sibling destination/device suite already has:
//! `tests/all/*.rs` are modules of ONE test target, so `router`, `request_signed_for`,
//! `call`, `error_code` and `is_uuid_shaped` are shared from there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The store ceiling the service enforces, so this suite asserts the bound the
/// implementation declares rather than a number copied by hand.
use service_com_amazonaws_iotwireless_iotwireless::state::{DEFAULT_REGION, MAX_DEVICE_PROFILES};

// The ONE wire harness this crate's suites share, and the region and account its
// signed requests mint ARNs in.
use super::destination_and_wireless_device_lifecycle::{
    call, error_code, is_uuid_shaped, request_signed_for, router, ACCOUNT, REGION,
};

/// `DeviceProfileType`'s modelled values — the model-generated constant, so the suite
/// and the service cannot drift apart.
const DEVICE_PROFILE_TYPES: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::DEVICE_PROFILE_TYPE_VALUES;

/// The characters every `Name` member's model doc excludes ("The following special
/// characters aren't accepted: <>^#~$").
const FORBIDDEN_NAME_CHARS: &[char] =
    service_com_amazonaws_iotwireless_iotwireless::state::FORBIDDEN_NAME_CHARS;

/// `LoRaWANDeviceProfile.PingSlotPeriod`'s published range.
/// Source: `smithy_query(query="shape", shape="LoRaWANDeviceProfile")`.
const PING_SLOT_PERIOD_MIN: i64 = 32;
const PING_SLOT_PERIOD_MAX: i64 = 4096;

/// `DeviceProfileName`'s published length ceiling.
/// Source: `smithy_query(query="operation", operation="CreateDeviceProfile")`.
const NAME_MAX_LEN: usize = 256;

/// Dispatch one call signed for `scope_region` — `None` leaves it unsigned, so a minted
/// ARN falls back to [`DEFAULT_REGION`] — and return its status plus its parsed JSON
/// body. Only a region table needs a scope other than [`REGION`]; every other case
/// goes through the shared `call`. Shared with the sibling suites that assert the same
/// region rule, so the dispatch exists once.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    scope_region: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = router
        .dispatch(request_signed_for(scope_region, method, uri, body))
        .await;
    let status = response.status();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_profile(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/device-profiles", Some(body)).await
}

async fn get_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/device-profiles/{id}"), None).await
}

/// Every member of `sent` must come back from the reader under `member`, unchanged.
fn assert_echoes(case: &str, member: &str, sent: &Value, fetched: &Value) {
    let sent: &Map<String, Value> = sent.as_object().expect("the sent member is an object");
    for (key, value) in sent {
        assert_eq!(
            fetched.get(member).and_then(|found| found.get(key)),
            Some(value),
            "case {case:?}: expected {member}.{key} = {value}, got {:?}",
            fetched.get(member).and_then(|found| found.get(key))
        );
    }
}

/// One create and the read that must report exactly what it stored.
struct Case {
    case: &'static str,
    name: Option<String>,
    /// The `LoRaWAN` member the create carries, if any.
    lorawan: Option<Value>,
    /// Whether the create carries the `Sidewalk` member.
    sidewalk: bool,
    /// The `DeviceProfileType` the stored profile is of.
    profile_type: &'static str,
    tags: Option<Value>,
}

fn cases() -> Vec<Case> {
    vec![
        // A LoRaWAN profile carrying a wide member set, each integer at a published
        // bound: the catalog entry the issue's units exercise.
        Case {
            case: "lorawan-full",
            name: Some("ClassB-Profile_1".to_string()),
            lorawan: Some(json!({
                "MacVersion": "1.0.3",
                "RegParamsRevision": "RP002-1.0.1",
                "RfRegion": "EU868",
                "SupportsClassB": true,
                "ClassBTimeout": 1000,
                "PingSlotPeriod": PING_SLOT_PERIOD_MAX,
                "PingSlotDr": 15,
                "PingSlotFreq": 16700000,
                "SupportsClassC": false,
                "ClassCTimeout": 0,
                "RxDelay1": 1,
                "RxDrOffset1": 7,
                "RxDataRate2": 8,
                "RxFreq2": 1000000,
                "FactoryPresetFreqsList": [1000000, 16700000],
                "MaxEirp": 36,
                "MaxDutyCycle": 100,
                "SupportsJoin": true,
                "Supports32BitFCnt": true,
            })),
            sidewalk: false,
            profile_type: "LoRaWAN",
            tags: Some(json!([{ "Key": "stage", "Value": "test" }])),
        },
        // A LoRaWAN profile carrying only the other end of each bound, and no name.
        Case {
            case: "lorawan-low-bounds-no-name",
            name: None,
            lorawan: Some(json!({
                "MacVersion": "1.1",
                "RfRegion": "US915",
                "ClassBTimeout": 0,
                "PingSlotPeriod": PING_SLOT_PERIOD_MIN,
                "PingSlotDr": 0,
                "SupportsClassC": true,
                "ClassCTimeout": 1000,
                "MaxEirp": 0,
                "MaxDutyCycle": 0,
                "SupportsJoin": false,
            })),
            sidewalk: false,
            profile_type: "LoRaWAN",
            tags: None,
        },
        // A LoRaWAN profile with no settings at all: the member is present and empty.
        Case {
            case: "lorawan-empty",
            name: Some(String::new()),
            lorawan: Some(json!({})),
            sidewalk: false,
            profile_type: "LoRaWAN",
            tags: Some(json!([])),
        },
        // A Sidewalk profile: `SidewalkCreateDeviceProfile` models no members, so the
        // only thing stored about it is that it IS a Sidewalk profile.
        Case {
            case: "sidewalk",
            name: Some("Sidewalk Profile".to_string()),
            lorawan: None,
            sidewalk: true,
            profile_type: "Sidewalk",
            tags: Some(json!([
                { "Key": "owner", "Value": "fleet-a" },
                { "Key": "stage", "Value": "prod" },
            ])),
        },
        // Neither technology member: a profile with no settings, which the model's
        // two-value catalog leaves as a LoRaWAN one.
        Case {
            case: "no-technology-member",
            name: Some("x".repeat(NAME_MAX_LEN)),
            lorawan: None,
            sidewalk: false,
            profile_type: "LoRaWAN",
            tags: None,
        },
    ]
}

/// The body a case sends to `CreateDeviceProfile`.
fn body_for(case: &Case) -> Value {
    let mut body = json!({});
    if let Some(name) = &case.name {
        body["Name"] = json!(name);
    }
    if let Some(lorawan) = &case.lorawan {
        body["LoRaWAN"] = lorawan.clone();
    }
    if case.sidewalk {
        body["Sidewalk"] = json!({});
    }
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

/// A create persists the profile and `GetDeviceProfile` reports exactly what was
/// stored, across the whole `DeviceProfileType` catalog, both presence states of every
/// optional member and both ends of the published integer ranges.
#[tokio::test]
async fn a_device_profile_round_trips_through_get_for_every_profile_type() {
    let router = router();
    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        assert!(
            DEVICE_PROFILE_TYPES.contains(&case.profile_type),
            "case {:?}: {:?} is not in the modelled catalog {DEVICE_PROFILE_TYPES:?}",
            case.case,
            case.profile_type
        );

        let (status, created) = create_profile(&router, body_for(&case)).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: expected 201, got {status} {created}",
            case.case
        );

        let id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Id in {created}", case.case))
            .to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {:?}: expected a uuid Id, got {id:?}",
            case.case
        );
        assert!(
            !minted.contains(&id),
            "case {:?}: Id {id:?} was already minted for another profile",
            case.case
        );
        let expected_arn = format!("arn:aws:iotwireless:{REGION}:{ACCOUNT}:DeviceProfile/{id}");
        assert_eq!(
            created["Arn"],
            json!(expected_arn),
            "case {:?}: expected Arn {expected_arn}, got {:?}",
            case.case,
            created["Arn"]
        );
        minted.push(id.clone());

        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {:?}: expected 200 from GetDeviceProfile, got {status} {fetched}",
            case.case
        );
        assert_eq!(
            fetched["Id"],
            json!(id),
            "case {:?}: expected Id {id:?}, got {:?}",
            case.case,
            fetched["Id"]
        );
        assert_eq!(
            fetched["Arn"],
            json!(expected_arn),
            "case {:?}: GetDeviceProfile Arn must match the created ARN, got {:?}",
            case.case,
            fetched["Arn"]
        );
        assert_eq!(
            fetched.get("Name").cloned(),
            case.name.as_ref().map(|name| json!(name)),
            "case {:?}: expected Name {:?}, got {:?}",
            case.case,
            case.name,
            fetched.get("Name")
        );

        if let Some(lorawan) = &case.lorawan {
            assert!(
                fetched.get("LoRaWAN").is_some(),
                "case {:?}: expected the stored LoRaWAN settings, got {fetched}",
                case.case
            );
            assert_echoes(case.case, "LoRaWAN", lorawan, &fetched);
        } else {
            assert!(
                fetched.get("LoRaWAN").is_none(),
                "case {:?}: a profile created without LoRaWAN settings must report none, got {:?}",
                case.case,
                fetched.get("LoRaWAN")
            );
        }
        assert_eq!(
            fetched.get("Sidewalk").is_some(),
            case.profile_type == "Sidewalk",
            "case {:?}: a {} profile reporting Sidewalk {:?} is wrong",
            case.case,
            case.profile_type,
            fetched.get("Sidewalk")
        );
    }
}

/// A minted ARN carries the region the request's SigV4 credential scope names, and
/// falls back to the default region when the request names none.
#[tokio::test]
async fn a_minted_arn_carries_the_region_the_request_names() {
    let router = router();

    for scope_region in [Some("us-east-2"), Some("ap-south-1"), None] {
        let (status, created) = call_signed_for(
            &router,
            scope_region,
            Method::POST,
            "/device-profiles",
            Some(json!({ "LoRaWAN": { "RfRegion": "EU868" } })),
        )
        .await;
        let case = scope_region.unwrap_or("<unsigned>");
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().unwrap_or_default();
        let expected_region = scope_region.unwrap_or(DEFAULT_REGION);
        assert_eq!(
            created["Arn"],
            json!(format!(
                "arn:aws:iotwireless:{expected_region}:{ACCOUNT}:DeviceProfile/{id}"
            )),
            "case {case:?}: expected the profile ARN in {expected_region}, got {:?}",
            created["Arn"]
        );
    }
}

/// `GetDeviceProfile` answers 404 for an `Id` the store does not hold — whatever the
/// id looks like — while the stored profile keeps resolving.
#[tokio::test]
async fn an_unknown_device_profile_id_is_not_found() {
    let router = router();
    let (_, created) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
    ] {
        let (status, body) = get_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {id:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    let (status, _) = get_profile(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored profile must still resolve after the refused reads"
    );
}

/// Replaying a `ClientRequestToken` with the same parameters returns the profile the
/// first call minted; replaying it with different parameters is a 409 that changes
/// nothing.
#[tokio::test]
async fn replaying_a_client_request_token_is_idempotent() {
    let router = router();

    for case in cases() {
        let token = format!("token-{}", case.case);
        let mut body = body_for(&case);
        body["ClientRequestToken"] = json!(token);

        let (status, first) = create_profile(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: first create, got {status} {first}",
            case.case
        );

        let (status, replay) = create_profile(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {:?}: a replay with the same parameters must succeed, got {status} {replay}",
            case.case
        );
        assert_eq!(
            replay["Id"], first["Id"],
            "case {:?}: a replay must return the first Id {:?}, got {:?}",
            case.case, first["Id"], replay["Id"]
        );
        assert_eq!(
            replay["Arn"], first["Arn"],
            "case {:?}: a replay must return the first Arn {:?}, got {:?}",
            case.case, first["Arn"], replay["Arn"]
        );

        let mut changed = body.clone();
        changed["Name"] = json!(format!("Changed-{}", case.case));
        let (status, conflict) = create_profile(&router, changed).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {:?}: a replay with different parameters must be 409, got {status} {conflict}",
            case.case
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {:?}: expected ConflictException, got {conflict}",
            case.case
        );

        let id = first["Id"].as_str().expect("an Id");
        let (_, fetched) = get_profile(&router, id).await;
        assert_eq!(
            fetched.get("Name").cloned(),
            case.name.as_ref().map(|name| json!(name)),
            "case {:?}: the refused create must not have overwritten Name, got {:?}",
            case.case,
            fetched.get("Name")
        );
    }
}

/// A name carrying any character the model doc excludes is refused, and nothing is
/// stored for it; the same name without that character is accepted.
#[tokio::test]
async fn a_name_with_an_excluded_character_is_refused() {
    let router = router();

    for rejected in FORBIDDEN_NAME_CHARS {
        for name in [
            format!("{rejected}LeadingProfile"),
            format!("Profile{rejected}Middle"),
            format!("TrailingProfile{rejected}"),
        ] {
            let (status, body) =
                create_profile(&router, json!({ "Name": name, "LoRaWAN": {} })).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {name:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: expected ValidationException, got {body}"
            );
            assert!(
                body.get("Id").is_none(),
                "case {name:?}: a refused create must mint nothing, got {body}"
            );

            let accepted: String = name.chars().filter(|c| c != rejected).collect();
            let (status, created) =
                create_profile(&router, json!({ "Name": accepted, "LoRaWAN": {} })).await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {accepted:?}: the same name without {rejected:?} must be accepted, \
                 got {status} {created}"
            );
        }
    }
}

/// A profile is of exactly one `DeviceProfileType`, so a create naming both
/// technologies is refused rather than stored as one of them.
#[tokio::test]
async fn a_create_naming_both_technologies_is_refused() {
    let router = router();

    for lorawan in [json!({}), json!({ "RfRegion": "EU868", "MacVersion": "1.0.3" })] {
        let (status, body) = create_profile(
            &router,
            json!({ "Name": "Both", "LoRaWAN": lorawan.clone(), "Sidewalk": {} }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {lorawan}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {lorawan}: expected ValidationException, got {body}"
        );
    }
}

/// `PingSlotPeriod` is accepted on each published bound and refused one past either
/// end, and a refusal stores nothing.
#[tokio::test]
async fn a_ping_slot_period_outside_the_published_range_is_refused() {
    let router = router();

    for (value, accepted) in [
        (PING_SLOT_PERIOD_MIN, true),
        (PING_SLOT_PERIOD_MIN + 1, true),
        (PING_SLOT_PERIOD_MAX, true),
        (PING_SLOT_PERIOD_MIN - 1, false),
        (PING_SLOT_PERIOD_MAX + 1, false),
    ] {
        let (status, body) = create_profile(
            &router,
            json!({ "LoRaWAN": { "PingSlotPeriod": value, "SupportsClassB": true } }),
        )
        .await;
        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case PingSlotPeriod={value}: expected 201, got {status} {body}"
            );
            let (status, fetched) =
                get_profile(&router, body["Id"].as_str().expect("an Id")).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case PingSlotPeriod={value}: expected 200 from the read, got {status} {fetched}"
            );
            assert_eq!(
                fetched["LoRaWAN"]["PingSlotPeriod"],
                json!(value),
                "case PingSlotPeriod={value}: expected it read back, got {:?}",
                fetched["LoRaWAN"]["PingSlotPeriod"]
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case PingSlotPeriod={value}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case PingSlotPeriod={value}: expected ValidationException, got {body}"
            );
        }
    }
}

/// The device-profile store is BOUNDED: the create refuses once it is full, and every
/// profile it accepted up to that point still reads back.
#[tokio::test]
async fn the_device_profile_store_is_bounded() {
    let router = router();
    let mut ids = Vec::new();

    for index in 0..MAX_DEVICE_PROFILES {
        let (status, body) = create_profile(
            &router,
            json!({ "Name": format!("Bounded{index}"), "LoRaWAN": {} }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {index}: expected 201 below the ceiling, got {status} {body}"
        );
        ids.push(body["Id"].as_str().expect("an Id").to_string());
    }

    let (status, body) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a full store must refuse, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a full store must refuse with ValidationException, got {body}"
    );

    for (index, id) in ids.iter().enumerate() {
        let (status, fetched) = get_profile(&router, id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {index}: profile {id} must still read back, got {status} {fetched}"
        );
    }
}
