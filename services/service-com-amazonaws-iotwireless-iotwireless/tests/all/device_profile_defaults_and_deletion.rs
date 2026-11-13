// SPDX-License-Identifier: CC0-1.0

//! What a device profile's LoRaWAN settings are after a create, and what a read of a
//! DELETED profile answers — end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a create keeps every LoRaWAN member it was given EXACTLY as submitted, and fills
//!   each setting the request left out with what the service has for it, so the read
//!   reports the settings the profile really has;
//! * a setting ONE band's regional plan decides — the second receive window's data rate
//!   and frequency, and the join channels — is taken from the band the profile resolves
//!   to, and a band the service holds no plan for gets none of the three rather than
//!   another band's channel;
//! * the members that describe the device's own capabilities are left unset when the
//!   request names none — a default is filled in only where this service has one;
//! * a delete removes the record, so every later read of that `Id` is refused with
//!   HTTP 404 `ResourceNotFoundException` carrying this service's not-found sentence,
//!   the requested `Id`, the resource kind and the error-type header;
//! * an `Id` no profile holds is refused the same way by the delete itself, and a
//!   refused call changes nothing.
//!
//! The wire harness is the one the sibling suites already have: `tests/all/*.rs` are
//! modules of ONE test target, so `router`, `request_signed_for`, `call` and
//! `error_code` are shared from there rather than copied.

use http::{HeaderMap, Method, StatusCode};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The band-independent defaults the service fills an unspecified device-profile
/// setting with — the service's own constants, so this suite and the implementation
/// cannot drift apart. Their published values are pinned by
/// [`the_band_independent_defaults_are_the_published_values`].
use service_com_amazonaws_iotwireless_iotwireless::state::{
    DEFAULT_DEVICE_PROFILE_RF_REGION, DEFAULT_MAX_DUTY_CYCLE, DEFAULT_RX_DELAY1,
    DEFAULT_RX_DR_OFFSET1, DEFAULT_SUPPORTS_JOIN,
};

/// `SupportedRfRegion`'s modelled values — the model-generated constant, so the frequency
/// band the service defaults to is checked against the model rather than a copy.
const SUPPORTED_RF_REGIONS: &[&str] =
    service_com_amazonaws_iotwireless_iotwireless::ops::types::SUPPORTED_RF_REGION_VALUES;

use super::destination_and_wireless_device_lifecycle::{
    call, error_code, request_signed_for, router, REGION,
};

/// `FactoryPresetFreqsList`'s published length ceiling, and `PresetFreq`'s published
/// range.
/// Source: `smithy_query(query="shape", shape="LoRaWANDeviceProfile,FactoryPresetFreqsList")`.
const FACTORY_PRESET_FREQS_MAX_LEN: usize = 20;
const PRESET_FREQ_MIN: i64 = 1_000_000;
const PRESET_FREQ_MAX: i64 = 16_700_000;

/// The published ranges of the integer members the service defaults.
/// Source: the same `LoRaWANDeviceProfile` shape query.
const MAX_DUTY_CYCLE_RANGE: (i64, i64) = (0, 100);
const RX_DELAY1_RANGE: (i64, i64) = (0, 15);
const RX_DR_OFFSET1_RANGE: (i64, i64) = (0, 7);
const RX_DATA_RATE2_RANGE: (i64, i64) = (0, 15);

/// The resource kind a device-profile refusal names.
const RESOURCE_TYPE: &str = "DeviceProfile";

/// What each regional plan publishes for the three members ONE band decides, as
/// `(SupportedRfRegion, RxDataRate2, RxFreq2, default join channels)`, every frequency
/// in the hundreds-of-hertz unit `RxFreq2`/`PresetFreq` are modelled in.
///
/// Written here from the published plans rather than read from the service, so the test
/// states what AWS reports instead of agreeing with the implementation: each second
/// receive window is the plan's RX2 channel (869.525 MHz at DR0 for EU868, 923.3 MHz at
/// DR8 for the 500 kHz plans, 923.2 MHz at DR2 for AS923-1 shifted by that plan's
/// published -6.60 MHz offset for AS923-3, 866.55 MHz at DR2 for IN865), and each
/// channel list is the band's default join channels. Rows cover bands the issue's units
/// never name, and CN470 is the band whose published channel set does not fit
/// `FactoryPresetFreqsList`'s twenty entries, so a profile there reports no list.
const PUBLISHED_PLANS: &[(&str, i64, i64, &[i64])] = &[
    (
        "US915",
        8,
        9_233_000,
        &[
            9_039_000, 9_041_000, 9_043_000, 9_045_000, 9_047_000, 9_049_000, 9_051_000,
            9_053_000, 9_046_000,
        ],
    ),
    ("EU868", 0, 8_695_250, &[8_681_000, 8_683_000, 8_685_000]),
    ("AS923-1", 2, 9_232_000, &[9_232_000, 9_234_000]),
    ("AS923-3", 2, 9_166_000, &[9_166_000, 9_168_000]),
    ("IN865", 2, 8_665_500, &[8_650_625, 8_654_025, 8_659_850]),
    ("EU433", 0, 4_346_650, &[4_331_750, 4_333_750, 4_335_750]),
    ("CN470", 0, 5_053_000, &[]),
];

/// The plan [`PUBLISHED_PLANS`] states for `band`, if this suite lists it.
fn published_plan(band: &str) -> Option<(i64, i64, &'static [i64])> {
    PUBLISHED_PLANS
        .iter()
        .find(|(region, _, _, _)| *region == band)
        .map(|(_, data_rate, freq, channels)| (*data_rate, *freq, *channels))
}

async fn create_profile(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/device-profiles", Some(body)).await
}

async fn get_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(router, Method::GET, &format!("/device-profiles/{id}"), None).await
}

async fn delete_profile(router: &MultiServiceRouter, id: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::DELETE,
        &format!("/device-profiles/{id}"),
        None,
    )
    .await
}

/// Dispatch one call and return its status, its response HEADERS and its parsed body —
/// the refusal cases assert the error-type header, which the shared `call` drops.
async fn call_with_headers(
    router: &MultiServiceRouter,
    method: Method,
    uri: &str,
) -> (StatusCode, HeaderMap, Value) {
    let response = router
        .dispatch(request_signed_for(Some(REGION), method, uri, None))
        .await;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

/// The members this service fills in whatever band the profile is in, each with the
/// value it is filled with.
fn band_independent_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("MaxDutyCycle", json!(DEFAULT_MAX_DUTY_CYCLE)),
        ("SupportsJoin", json!(DEFAULT_SUPPORTS_JOIN)),
        ("RxDelay1", json!(DEFAULT_RX_DELAY1)),
        ("RxDrOffset1", json!(DEFAULT_RX_DR_OFFSET1)),
    ]
}

/// The members one band's regional plan decides, so a profile takes them from the band
/// it resolves to rather than from a fixed set.
const BAND_DEPENDENT_MEMBERS: &[&str] = &["RxDataRate2", "RxFreq2", "FactoryPresetFreqsList"];

/// The members that describe the device's own capabilities rather than the frequency
/// plan: this service has no default for them, so a profile reports one only when its
/// create named it.
const NOT_DEFAULTED_MEMBERS: &[&str] = &[
    "MacVersion",
    "RegParamsRevision",
    "MaxEirp",
    "ClassBTimeout",
    "ClassCTimeout",
    "PingSlotPeriod",
    "PingSlotDr",
    "PingSlotFreq",
];

/// The `@default false` booleans the model declares, which a profile reports either way.
/// Source: `smithy_query(query="shape", shape="LoRaWANDeviceProfile")`.
const DEFAULT_FALSE_BOOLEANS: &[&str] =
    &["SupportsClassB", "SupportsClassC", "Supports32BitFCnt"];

/// The LoRaWAN block the read must report for a create that submitted `sent`: every
/// submitted member unchanged, the band-independent default for each one it omitted, and
/// — for a member the regional plan decides — the plan of the band the profile resolves
/// to, which is the band the request named or the one a profile defaults to.
fn expected_block(sent: &Value) -> Map<String, Value> {
    let sent = sent.as_object().expect("the LoRaWAN member is an object");
    let mut expected = sent.clone();
    for (member, default) in band_independent_defaults() {
        expected
            .entry(member.to_string())
            .or_insert_with(|| default.clone());
    }
    let band = sent
        .get("RfRegion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_DEVICE_PROFILE_RF_REGION)
        .to_string();
    if let Some((data_rate, freq, channels)) = published_plan(&band) {
        expected
            .entry("RxDataRate2".to_string())
            .or_insert_with(|| json!(data_rate));
        expected
            .entry("RxFreq2".to_string())
            .or_insert_with(|| json!(freq));
        if !channels.is_empty() {
            expected
                .entry("FactoryPresetFreqsList".to_string())
                .or_insert_with(|| json!(channels));
        }
    }
    expected.entry("RfRegion".to_string()).or_insert(json!(band));
    expected
}

/// The defaults no band decides, and the modelled constraint each satisfies: the band a
/// profile takes when its create names none is one the model lists, and each filled
/// scalar is inside its published range.
#[test]
fn the_band_independent_defaults_are_the_published_values() {
    assert!(
        SUPPORTED_RF_REGIONS.contains(&DEFAULT_DEVICE_PROFILE_RF_REGION),
        "the default frequency band must be a modelled SupportedRfRegion value, \
         {SUPPORTED_RF_REGIONS:?} does not list {DEFAULT_DEVICE_PROFILE_RF_REGION:?}"
    );
    assert_eq!(
        json!(DEFAULT_SUPPORTS_JOIN),
        json!(true),
        "a defaulted profile supports over-the-air activation, got {DEFAULT_SUPPORTS_JOIN}"
    );

    for (case, value, (min, max)) in [
        (
            "MaxDutyCycle",
            i64::from(DEFAULT_MAX_DUTY_CYCLE),
            MAX_DUTY_CYCLE_RANGE,
        ),
        ("RxDelay1", i64::from(DEFAULT_RX_DELAY1), RX_DELAY1_RANGE),
        (
            "RxDrOffset1",
            i64::from(DEFAULT_RX_DR_OFFSET1),
            RX_DR_OFFSET1_RANGE,
        ),
    ] {
        assert!(
            (min..=max).contains(&value),
            "case {case:?}: the default {value} is outside the published range {min}..{max}"
        );
    }
}

/// The three members a regional plan decides are taken from the band the profile
/// resolves to: a create that names a band and omits them reports THAT band's published
/// second receive window and join channels, and a band this service holds no plan for
/// reports none of the three rather than another band's channel.
#[tokio::test]
async fn the_band_decides_the_receive_window_and_the_join_channels() {
    let router = router();

    // Every band this suite publishes a plan for, plus the band-less create (which
    // resolves to the default band) and a band string the regional catalog does not
    // list.
    let mut cases: Vec<(&str, Option<&str>)> = vec![("band-less", None), ("ZZ-NO-PLAN", Some("ZZ-NO-PLAN"))];
    cases.extend(PUBLISHED_PLANS.iter().map(|(band, _, _, _)| (*band, Some(*band))));

    for (case, band) in cases {
        let lorawan = match band {
            Some(band) => json!({ "RfRegion": band }),
            None => json!({}),
        };
        let (status, created) = create_profile(&router, json!({ "LoRaWAN": lorawan })).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id").to_string();
        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from the read, got {status} {fetched}"
        );
        let block = &fetched["LoRaWAN"];

        let resolved = band.unwrap_or(DEFAULT_DEVICE_PROFILE_RF_REGION);
        assert_eq!(
            block.get("RfRegion"),
            Some(&json!(resolved)),
            "case {case:?}: expected the resolved band {resolved:?}, got {:?}",
            block.get("RfRegion")
        );
        match published_plan(resolved) {
            Some((data_rate, freq, channels)) => {
                assert_eq!(
                    block.get("RxDataRate2"),
                    Some(&json!(data_rate)),
                    "case {case:?}: expected the band's RX2 data rate {data_rate}, got {:?}",
                    block.get("RxDataRate2")
                );
                assert_eq!(
                    block.get("RxFreq2"),
                    Some(&json!(freq)),
                    "case {case:?}: expected the band's RX2 frequency {freq}, got {:?}",
                    block.get("RxFreq2")
                );
                let expected_channels = (!channels.is_empty()).then(|| json!(channels));
                assert_eq!(
                    block.get("FactoryPresetFreqsList").cloned(),
                    expected_channels,
                    "case {case:?}: expected the band's join channels {:?}, got {:?}",
                    channels,
                    block.get("FactoryPresetFreqsList")
                );
            }
            // A band with no plan: the three members the plan decides are absent, while
            // the ones no band decides are still filled in.
            None => {
                for member in BAND_DEPENDENT_MEMBERS {
                    assert!(
                        block.get(*member).is_none_or(Value::is_null),
                        "case {case:?}: a band with no regional plan must report no \
                         {member}, got {:?}",
                        block.get(*member)
                    );
                }
                for (member, expected) in band_independent_defaults() {
                    assert_eq!(
                        block.get(member),
                        Some(&expected),
                        "case {case:?}: expected LoRaWAN.{member} = {expected}, got {:?}",
                        block.get(member)
                    );
                }
            }
        }
    }
}

/// EVERY band the model lists is answered from the regional catalog, and what it answers
/// satisfies the published constraints: the data rate is inside `RxDataRate2`'s range,
/// the frequency inside `RxFreq2`'s, and the channel list fits `FactoryPresetFreqsList`'s
/// ceiling with every channel inside `PresetFreq`'s range.
#[tokio::test]
async fn every_modelled_band_is_filled_within_the_published_constraints() {
    let router = router();

    for band in SUPPORTED_RF_REGIONS {
        let (status, created) =
            create_profile(&router, json!({ "LoRaWAN": { "RfRegion": band } })).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {band:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id").to_string();
        let (_, fetched) = get_profile(&router, &id).await;
        let block = &fetched["LoRaWAN"];

        let data_rate = block["RxDataRate2"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {band:?}: no RxDataRate2 in {block}"));
        assert!(
            (RX_DATA_RATE2_RANGE.0..=RX_DATA_RATE2_RANGE.1).contains(&data_rate),
            "case {band:?}: RxDataRate2 {data_rate} is outside the published range \
             {}..{}",
            RX_DATA_RATE2_RANGE.0,
            RX_DATA_RATE2_RANGE.1
        );
        let freq = block["RxFreq2"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {band:?}: no RxFreq2 in {block}"));
        assert!(
            (PRESET_FREQ_MIN..=PRESET_FREQ_MAX).contains(&freq),
            "case {band:?}: RxFreq2 {freq} is outside the published range \
             {PRESET_FREQ_MIN}..{PRESET_FREQ_MAX}"
        );
        let channels = block
            .get("FactoryPresetFreqsList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            channels.len() <= FACTORY_PRESET_FREQS_MAX_LEN,
            "case {band:?}: {} channels exceeds the published ceiling \
             {FACTORY_PRESET_FREQS_MAX_LEN}",
            channels.len()
        );
        for channel in &channels {
            let channel = channel
                .as_i64()
                .unwrap_or_else(|| panic!("case {band:?}: a non-integer channel in {block}"));
            assert!(
                (PRESET_FREQ_MIN..=PRESET_FREQ_MAX).contains(&channel),
                "case {band:?}: channel {channel} is outside the published range \
                 {PRESET_FREQ_MIN}..{PRESET_FREQ_MAX}"
            );
        }
    }
}

/// A create keeps every LoRaWAN member it was given and fills in what the service has
/// for each one it was not — the band-independent default, or the resolved band's plan —
/// across blocks that submit none, some and all of the filled members, with every
/// submitted value deliberately DIFFERENT from what would otherwise be filled in, so an
/// override cannot pass as an echo.
#[tokio::test]
async fn omitted_lorawan_settings_take_the_service_defaults() {
    let router = router();

    // (case, the `LoRaWAN` block the create carries)
    let cases = [
        // Nothing but the member itself: every default is filled in.
        ("empty-block", json!({})),
        // Only the frequency band, and not the one defaulted to.
        ("band-only", json!({ "RfRegion": "EU868" })),
        // Capability members only: they are kept, and every frequency-plan member is
        // still filled in.
        (
            "capabilities-only",
            json!({
                "MacVersion": "1.0.3",
                "RegParamsRevision": "RP002-1.0.1",
                "MaxEirp": 15,
                "SupportsClassB": true,
                "ClassBTimeout": 1000,
                "PingSlotPeriod": 32,
            }),
        ),
        // A subset of the defaulted members, each at a value the default is not.
        (
            "partial-overrides",
            json!({
                "RfRegion": "AU915",
                "MaxDutyCycle": 0,
                "SupportsJoin": false,
                "RxFreq2": PRESET_FREQ_MIN,
            }),
        ),
        // Every defaulted member submitted, each away from its default, the list at the
        // published length ceiling.
        (
            "every-member-submitted",
            json!({
                "RfRegion": "AS923-1",
                "MaxDutyCycle": MAX_DUTY_CYCLE_RANGE.1,
                "SupportsJoin": false,
                "RxDelay1": RX_DELAY1_RANGE.1,
                "RxDrOffset1": RX_DR_OFFSET1_RANGE.1,
                "RxDataRate2": RX_DATA_RATE2_RANGE.1,
                "RxFreq2": PRESET_FREQ_MAX,
                "FactoryPresetFreqsList": (0..FACTORY_PRESET_FREQS_MAX_LEN)
                    .map(|index| PRESET_FREQ_MIN + index as i64)
                    .collect::<Vec<_>>(),
            }),
        ),
        // An EMPTY submitted channel plan is a submitted value, not an omission: the
        // default plan must not replace it.
        (
            "empty-channel-plan",
            json!({ "FactoryPresetFreqsList": [] }),
        ),
        // One channel, at the other end of the published range.
        (
            "one-channel-plan",
            json!({ "FactoryPresetFreqsList": [PRESET_FREQ_MAX] }),
        ),
    ];

    for (case, sent) in cases {
        let (status, created) =
            create_profile(&router, json!({ "Name": case, "LoRaWAN": sent.clone() })).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {created}"))
            .to_string();

        let (status, fetched) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: expected 200 from GetDeviceProfile, got {status} {fetched}"
        );
        let block = fetched
            .get("LoRaWAN")
            .unwrap_or_else(|| panic!("case {case:?}: no LoRaWAN block in {fetched}"));

        for (member, expected) in expected_block(&sent) {
            assert_eq!(
                block.get(&member),
                Some(&expected),
                "case {case:?}: expected LoRaWAN.{member} = {expected}, got {:?}",
                block.get(&member)
            );
        }
        let sent_members = sent.as_object().expect("an object");
        for member in NOT_DEFAULTED_MEMBERS {
            if sent_members.contains_key(*member) {
                continue;
            }
            assert!(
                block.get(*member).is_none_or(Value::is_null),
                "case {case:?}: LoRaWAN.{member} has no default, so an omitting create \
                 must report none, got {:?}",
                block.get(*member)
            );
        }
        for member in DEFAULT_FALSE_BOOLEANS {
            let expected = sent_members
                .get(*member)
                .cloned()
                .unwrap_or(Value::Bool(false));
            assert_eq!(
                block.get(*member),
                Some(&expected),
                "case {case:?}: expected LoRaWAN.{member} = {expected}, got {:?}",
                block.get(*member)
            );
        }
    }
}

/// A channel plan longer than the published ceiling is refused, and nothing is stored
/// for it — the bound one past which the filled default could never be reported.
#[tokio::test]
async fn a_channel_plan_past_the_published_length_is_refused() {
    let router = router();

    for length in [
        FACTORY_PRESET_FREQS_MAX_LEN,
        FACTORY_PRESET_FREQS_MAX_LEN + 1,
    ] {
        let plan: Vec<i64> = (0..length)
            .map(|index| PRESET_FREQ_MIN + index as i64)
            .collect();
        let (status, body) = create_profile(
            &router,
            json!({ "LoRaWAN": { "FactoryPresetFreqsList": plan } }),
        )
        .await;
        if length <= FACTORY_PRESET_FREQS_MAX_LEN {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {length} channels: expected 201 at the ceiling, got {status} {body}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "case {length} channels: expected 400 past the ceiling, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {length} channels: expected ValidationException, got {body}"
            );
            assert!(
                body.get("Id").is_none(),
                "case {length} channels: a refused create must mint nothing, got {body}"
            );
        }
    }
}

/// The 404 body a device profile the account does not hold is refused with: the sentence
/// this service words for the class, with the resource kind and the requested id
/// interpolated, and both optional members carrying those same two values.
fn expected_refusal(id: &str) -> Value {
    json!({
        "Message": format!(
            "Cannot complete the step because resource is not found, with resource type \
             {RESOURCE_TYPE}, resource Id {id}"
        ),
        "ResourceId": id,
        "ResourceType": RESOURCE_TYPE,
    })
}

/// Assert `answered` is the `ResourceNotFoundException` a missing device profile `id` is
/// refused with — the status, the error-type header, the error code and all three
/// members.
fn assert_not_found(case: &str, status: StatusCode, headers: &HeaderMap, answered: &Value, id: &str) {
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "case {case:?}: expected 404, got {status} {answered}"
    );
    assert_eq!(
        error_code(answered),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answered}"
    );
    assert!(
        headers.contains_key("x-amzn-errortype"),
        "case {case:?}: expected the error-type header, got {:?}",
        headers.keys().collect::<Vec<_>>()
    );
    let expected = expected_refusal(id);
    for member in ["Message", "ResourceId", "ResourceType"] {
        assert_eq!(
            answered.get(member),
            expected.get(member),
            "case {case:?}: expected {member} = {:?}, got {:?}",
            expected.get(member),
            answered.get(member)
        );
    }
}

/// A delete is observable through the read path: the profile it removed is refused from
/// the next call on, whatever technology it was of and whether or not a client request
/// token minted it, while every other stored profile keeps resolving.
#[tokio::test]
async fn a_deleted_device_profile_is_refused_by_every_later_read() {
    let router = router();

    // (case, the create body)
    let cases = [
        (
            "lorawan-defaulted",
            json!({ "Name": "Defaulted", "LoRaWAN": {} }),
        ),
        (
            "lorawan-populated",
            json!({
                "Name": "Populated",
                "LoRaWAN": { "RfRegion": "EU868", "MacVersion": "1.0.3" },
                "Tags": [{ "Key": "stage", "Value": "test" }],
            }),
        ),
        ("sidewalk", json!({ "Name": "Sidewalk", "Sidewalk": {} })),
        ("no-technology-member", json!({ "Name": "Bare" })),
        (
            "minted-by-a-token",
            json!({ "LoRaWAN": {}, "ClientRequestToken": "delete-me-token" }),
        ),
    ];

    // A profile that is never deleted: every refusal above must leave it readable.
    let (_, bystander) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let bystander = bystander["Id"].as_str().expect("an Id").to_string();

    for (case, body) in cases {
        let (status, created) = create_profile(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: expected 201, got {status} {created}"
        );
        let id = created["Id"].as_str().expect("an Id").to_string();
        let (status, _) = get_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the profile must read back before the delete, got {status}"
        );

        let (status, body) = delete_profile(&router, &id).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "case {case:?}: expected 204 from DeleteDeviceProfile, got {status} {body}"
        );
        assert!(
            body.as_object().is_none_or(Map::is_empty),
            "case {case:?}: a 204 carries no members, got {body}"
        );

        let (status, headers, answered) =
            call_with_headers(&router, Method::GET, &format!("/device-profiles/{id}")).await;
        assert_not_found(case, status, &headers, &answered, &id);

        // Deleting it again is the same refusal: the record is gone, not tombstoned.
        let (status, headers, answered) =
            call_with_headers(&router, Method::DELETE, &format!("/device-profiles/{id}")).await;
        assert_not_found(case, status, &headers, &answered, &id);

        let (status, fetched) = get_profile(&router, &bystander).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the profile that was not deleted must still resolve, \
             got {status} {fetched}"
        );
    }
}

/// `DeleteDeviceProfile` refuses an `Id` the store does not hold — whatever the id looks
/// like — and a refused delete removes nothing.
#[tokio::test]
async fn deleting_an_unknown_device_profile_id_is_not_found() {
    let router = router();
    let (_, created) = create_profile(&router, json!({ "LoRaWAN": {} })).await;
    let present = created["Id"].as_str().expect("an Id").to_string();

    for id in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        "A",
        &present.to_uppercase(),
        &"x".repeat(256),
    ] {
        let (status, headers, answered) =
            call_with_headers(&router, Method::DELETE, &format!("/device-profiles/{id}")).await;
        assert_not_found(id, status, &headers, &answered, id);
    }

    let (status, fetched) = get_profile(&router, &present).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the stored profile must survive the refused deletes, got {status} {fetched}"
    );
}

/// A delete releases the `ClientRequestToken` that minted the profile: replaying the
/// token afterwards mints a NEW profile rather than replaying an id nothing holds.
#[tokio::test]
async fn deleting_a_profile_releases_the_token_that_minted_it() {
    let router = router();
    let body = json!({
        "Name": "Tokened",
        "LoRaWAN": { "RfRegion": "EU868" },
        "ClientRequestToken": "profile-token-1",
    });

    let (status, first) = create_profile(&router, body.clone()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "expected 201 from the first create, got {status} {first}"
    );
    let first_id = first["Id"].as_str().expect("an Id").to_string();

    let (status, _) = delete_profile(&router, &first_id).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "expected 204, got {status}");

    let (status, replay) = create_profile(&router, body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a replayed token whose profile was deleted must create one, got {status} {replay}"
    );
    let replayed_id = replay["Id"].as_str().expect("an Id").to_string();
    assert_ne!(
        replayed_id, first_id,
        "the replay must mint a fresh Id, got the deleted {first_id}"
    );

    let (status, fetched) = get_profile(&router, &replayed_id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the replayed create's profile must read back, got {status} {fetched}"
    );
    assert_eq!(
        fetched["LoRaWAN"]["RfRegion"],
        json!("EU868"),
        "the replayed create's settings must be stored, got {:?}",
        fetched["LoRaWAN"]
    );
}
