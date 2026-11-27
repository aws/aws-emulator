// SPDX-License-Identifier: CC0-1.0

//! How `CreateWirelessDevice` refuses an incoherent create, end to end over HTTP.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed `http::Request` — so what is asserted is what an SDK client sees, not
//! a handler return value.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * LoRaWAN key material is specification-version specific, so a create whose
//!   provisioning member (`OtaaV1_1`/`AbpV1_1` for 1.1, `OtaaV1_0_x`/`AbpV1_0_x` for
//!   1.0.x) names a version the referenced device profile's `MacVersion` family does
//!   not is refused `ValidationException` and persists nothing, while every create
//!   whose two versions agree — and every create that constrains neither side — is
//!   accepted and reads back against that profile;
//! * a refusal carries the sentence of the collision it reports, so a create that collides
//!   on a `DevEui` answers the already-claimed sentence when it re-presents the join key
//!   material the registered device was registered with, and the duplicate-identifier
//!   sentence when that material differs — each the same across every other difference
//!   between the two requests, neither disclosing anything about the device that holds the
//!   identifier, and both distinct from the sentence the other `ConflictException` of the
//!   same operation carries;
//! * the already-claimed sentence's one variable part is the `DevEui` the request submitted,
//!   so a client can recover the colliding identifier from the text and no two identifiers
//!   answer the same sentence.
//!
//! The wire harness is the one the destination/device suite already has: `tests/all/*.rs`
//! are modules of ONE test target, so `router`, `call` and `error_code` are shared from
//! there rather than copied.

use http::{Method, StatusCode};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::destination_and_wireless_device_lifecycle::{call, error_code, router};

/// The sentence a create gets when it claims a `DevEui` a DIFFERENT device holds: FIXED text,
/// with no variable part — neither the identifier the request submitted nor the device that
/// holds it — spelled out here so the suite pins the wording rather than whatever the service
/// happens to emit.
const DUPLICATE_DEV_EUI_MESSAGE: &str = "Device with given DevEui already exists";

/// The two fixed halves of the already-claimed sentence, between which the submitted
/// identifier sits. The trailing double space of the prefix belongs to the template.
const CLAIMED_DEV_EUI_PREFIX: &str = "DevEui ";
const CLAIMED_DEV_EUI_SUFFIX: &str = "  with same key has been claimed";

/// `DevEui` values spanning what the model admits: sixteen hex digits in either case.
/// Source: `smithy_query(query="shape", shape="DevEui")` — `pattern ^[a-fA-F0-9]{16}$`.
/// The rows differ along the one thing the already-claimed sentence's variable part
/// generalises over, the identifier's value: hex case, the two extremes of the range, and
/// digits only.
const DEV_EUI_CASES: &[(&str, &str)] = &[
    ("lowercase hex", "bbb78a67f1193555"),
    ("uppercase hex", "ABCDEF0123456789"),
    ("mixed-case hex", "aBcDeF9876543210"),
    ("all-zero, the low extreme", "0000000000000000"),
    ("all-f, the high extreme", "ffffffffffffffff"),
    ("digits only", "1234567890123456"),
];

/// The sentence a create gets when it re-presents the `DevEui` AND the join key material of a
/// device already registered: the identifier it was claimed under is its only variable part.
fn claimed_dev_eui_message(dev_eui: &str) -> String {
    format!("{CLAIMED_DEV_EUI_PREFIX}{dev_eui}{CLAIMED_DEV_EUI_SUFFIX}")
}

/// AWS's sentence for key material the referenced device profile's `MacVersion` cannot
/// provision. The only variable part is that stored `MacVersion`.
fn mac_version_mismatch_message(mac_version: &str) -> String {
    format!(
        "Validation error: the input profile version is not match macVersion defines in \
         deviceProfile: {mac_version}"
    )
}

async fn seed_destination(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        Method::POST,
        "/destinations",
        Some(json!({
            "Name": name,
            "ExpressionType": "RuleName",
            "Expression": format!("rule-for-{name}"),
            "RoleArn": "arn:aws:iam::000000000000:role/iotwireless-destination",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "seeding destination {name}: expected 201, got {status} {body}"
    );
    name.to_string()
}

/// Create a LoRaWAN device profile whose `LoRaWAN` member is `lorawan`, returning its
/// minted `Id`.
async fn seed_device_profile(router: &MultiServiceRouter, case: &str, lorawan: Value) -> String {
    let (status, body) = call(
        router,
        Method::POST,
        "/device-profiles",
        Some(json!({ "LoRaWAN": lorawan })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "case {case:?}: seeding the device profile expected 201, got {status} {body}"
    );
    body["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the created profile must carry an Id: {body}"))
        .to_string()
}

async fn create_device(router: &MultiServiceRouter, body: Value) -> (StatusCode, Value) {
    call(router, Method::POST, "/wireless-devices", Some(body)).await
}

/// `GetWirelessDevice` by `DevEui`, which is how a refused create is shown to have
/// persisted nothing.
async fn get_by_dev_eui(router: &MultiServiceRouter, dev_eui: &str) -> (StatusCode, Value) {
    call(
        router,
        Method::GET,
        &format!("/wireless-devices/{dev_eui}?identifierType=DevEui"),
        None,
    )
    .await
}

/// One LoRaWAN 1.1 key-material member and one 1.0.x member of each provisioning kind,
/// with a body the model's shapes accept.
/// Source: `smithy_query(query="shape", shape="LoRaWANDevice")` — `OtaaV1_1`,
/// `OtaaV1_0_x`, `AbpV1_1`, `AbpV1_0_x`.
fn provisioning_member(member: &str) -> Value {
    match member {
        "OtaaV1_1" => json!({
            "AppKey": "00000000000000000000000000000011",
            "NwkKey": "00000000000000000000000000000012",
            "JoinEui": "0000000000000013",
        }),
        "OtaaV1_0_x" => json!({
            "AppKey": "00000000000000000000000000000021",
            "AppEui": "0000000000000022",
        }),
        "AbpV1_1" => json!({
            "DevAddr": "00000031",
            "SessionKeys": {
                "FNwkSIntKey": "00000000000000000000000000000032",
                "SNwkSIntKey": "00000000000000000000000000000033",
                "NwkSEncKey": "00000000000000000000000000000034",
                "AppSKey": "00000000000000000000000000000035",
            },
        }),
        "AbpV1_0_x" => json!({
            "DevAddr": "00000041",
            "SessionKeys": {
                "NwkSKey": "00000000000000000000000000000042",
                "AppSKey": "00000000000000000000000000000043",
            },
        }),
        other => panic!("no provisioning body for member {other:?}"),
    }
}

/// One create: the profile's `MacVersion` (`None` when the profile declares none), the
/// provisioning members the request carries (empty when it carries none), and whether
/// every one of them agrees with the profile.
struct VersionCase {
    case: &'static str,
    mac_version: Option<&'static str>,
    provisioning: &'static [&'static str],
    accepted: bool,
}

/// The `MacVersion` values below are the LoRaWAN MAC revisions a device profile
/// declares; `1.0` and `1.1` are the two families, `1.0.2`/`1.0.3`/`1.0.4` revisions of
/// the first.
/// Source: `LoRaWANDeviceProfile.MacVersion`'s model doc ("The MAC version (such as
/// OTAA 1.1 or OTAA 1.0.3) to use with this device profile").
fn version_cases() -> Vec<VersionCase> {
    vec![
        VersionCase {
            case: "OtaaV1_1 against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_1 against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["OtaaV1_1"],
            accepted: false,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["OtaaV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.0 profile",
            mac_version: Some("1.0"),
            provisioning: &["OtaaV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_0_x against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_1 against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["AbpV1_1"],
            accepted: true,
        },
        VersionCase {
            case: "AbpV1_1 against a 1.0.2 profile",
            mac_version: Some("1.0.2"),
            provisioning: &["AbpV1_1"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_0_x against a 1.0.4 profile",
            mac_version: Some("1.0.4"),
            provisioning: &["AbpV1_0_x"],
            accepted: true,
        },
        VersionCase {
            case: "AbpV1_0_x against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["AbpV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "OtaaV1_1 and AbpV1_1 together against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1", "AbpV1_1"],
            accepted: true,
        },
        // Key material for two families cannot agree with a profile that declares one,
        // whichever of the two the profile names.
        VersionCase {
            case: "OtaaV1_1 and OtaaV1_0_x together against a 1.1 profile",
            mac_version: Some("1.1"),
            provisioning: &["OtaaV1_1", "OtaaV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "AbpV1_1 and AbpV1_0_x together against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &["AbpV1_1", "AbpV1_0_x"],
            accepted: false,
        },
        VersionCase {
            case: "no provisioning member against a 1.0.3 profile",
            mac_version: Some("1.0.3"),
            provisioning: &[],
            accepted: true,
        },
        VersionCase {
            case: "OtaaV1_1 against a profile that declares no MacVersion",
            mac_version: None,
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
        // `MacVersion` is `length 0..64`, so the empty string declares nothing and
        // constrains nothing.
        VersionCase {
            case: "OtaaV1_1 against a profile whose MacVersion is empty",
            mac_version: Some(""),
            provisioning: &["OtaaV1_1"],
            accepted: true,
        },
    ]
}

/// The key material a create carries must be the version the referenced device
/// profile's `MacVersion` provisions: a mismatch is a 400 that persists nothing, a match
/// is a 201 that reads back against that profile.
#[tokio::test]
async fn provisioning_version_must_agree_with_the_referenced_profiles_mac_version() {
    let router = router();
    seed_destination(&router, "VersionDest").await;

    for (index, case) in version_cases().into_iter().enumerate() {
        let VersionCase {
            case,
            mac_version,
            provisioning,
            accepted,
        } = case;

        let profile_lorawan = match mac_version {
            Some(version) => json!({ "MacVersion": version, "RegParamsRevision": "RP002-1.0.1" }),
            None => json!({ "RegParamsRevision": "RP002-1.0.1" }),
        };
        let profile_id = seed_device_profile(&router, case, profile_lorawan).await;

        let dev_eui = format!("00000000000001{index:02x}");
        let mut lorawan = json!({
            "DevEui": dev_eui,
            "DeviceProfileId": profile_id,
        });
        for member in provisioning {
            lorawan[*member] = provisioning_member(member);
        }
        let (status, response) = create_device(
            &router,
            json!({
                "Type": "LoRaWAN",
                "DestinationName": "VersionDest",
                "LoRaWAN": lorawan,
            }),
        )
        .await;

        if accepted {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "case {case:?}: expected 201, got {status} {response}"
            );
            let id = response["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {response}"))
                .to_string();
            let (status, fetched) = call(
                &router,
                Method::GET,
                &format!("/wireless-devices/{id}?identifierType=WirelessDeviceId"),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "case {case:?}: the accepted device must read back, got {status} {fetched}"
            );
            assert_eq!(
                fetched.pointer("/LoRaWAN/DeviceProfileId"),
                Some(&json!(profile_id)),
                "case {case:?}: expected DeviceProfileId {profile_id:?}, got {:?}",
                fetched.pointer("/LoRaWAN/DeviceProfileId")
            );
            continue;
        }

        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let expected = mac_version_mismatch_message(mac_version.unwrap_or_default());
        assert_eq!(
            response["Message"],
            json!(expected),
            "case {case:?}: expected Message {expected:?}, got {response}"
        );

        // A refused create persisted nothing: the rejected DevEui resolves to no device,
        // and no entry of the list carries it.
        let (status, fetched) = get_by_dev_eui(&router, &dev_eui).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "case {case:?}: the refused DevEui must resolve to nothing, got {status} {fetched}"
        );
        let (_, listed) = call(&router, Method::GET, "/wireless-devices", None).await;
        let carries_rejected = listed["WirelessDeviceList"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.pointer("/LoRaWAN/DevEui") == Some(&json!(dev_eui)))
            })
            .unwrap_or(false);
        assert!(
            !carries_rejected,
            "case {case:?}: the refused device must be absent from the list, got {listed}"
        );
    }
}

/// One pair of creates that collide on a `DevEui`: the key material the registered device
/// carries, the material the second create presents, the members that second request carries
/// beside its `LoRaWAN` block, and whether the two present the SAME join key material.
struct ConflictCase {
    case: &'static str,
    dev_eui: &'static str,
    holder_material: Value,
    claimant_material: Value,
    other_members: Value,
    same_key: bool,
}

/// The rows span what the refusal's sentence turns on — whether the claimant re-presents the
/// registered device's own join key material — across every provisioning block the model
/// declares, the hex case the material is written in, and what else the second request
/// carries.
/// Source for the blocks and their members: `smithy_query(query="shape",
/// shape="LoRaWANDevice,OtaaV1_1,OtaaV1_0_x,AbpV1_1,AbpV1_0_x")`.
fn conflict_cases() -> Vec<ConflictCase> {
    vec![
        ConflictCase {
            case: "neither create presents key material",
            dev_eui: "00000000000000a1",
            holder_material: json!({}),
            claimant_material: json!({}),
            other_members: json!({}),
            same_key: true,
        },
        ConflictCase {
            case: "the same OTAA 1.0.x material, under a different name and destination",
            dev_eui: "ABCDEF0123456789",
            holder_material: json!({ "OtaaV1_0_x": provisioning_member("OtaaV1_0_x") }),
            claimant_material: json!({ "OtaaV1_0_x": provisioning_member("OtaaV1_0_x") }),
            other_members: json!({ "Name": "second", "DestinationName": "ConflictDestB" }),
            same_key: true,
        },
        ConflictCase {
            // Every key member is hexadecimal (`^[a-fA-F0-9]{32}$`), and hexadecimal is
            // case-insensitive, so the same key written in the other case is the same key.
            case: "the same OTAA 1.1 material written in the other hex case, described and \
                   tagged",
            dev_eui: "00000000000000a2",
            holder_material: json!({ "OtaaV1_1": {
                "AppKey": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab1",
                "NwkKey": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab2",
                "JoinEui": "aaaaaaaaaaaaaab3",
            } }),
            claimant_material: json!({ "OtaaV1_1": {
                "AppKey": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB1",
                "NwkKey": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB2",
                "JoinEui": "AAAAAAAAAAAAAAB3",
            } }),
            other_members: json!({
                "Description": "a second claimant",
                "Tags": [{ "Key": "owner", "Value": "second" }],
            }),
            same_key: true,
        },
        ConflictCase {
            // `FCntStart` is the frame counter a session resumes from, not key material: it
            // moves over a device's life, so it does not decide which conflict this is.
            case: "ABP 1.1 material differing only in the frame counter a session resumes from",
            dev_eui: "ffffffffffffffff",
            holder_material: json!({ "AbpV1_1": { "DevAddr": "000000b1", "FCntStart": 0,
                "SessionKeys": provisioning_member("AbpV1_1")["SessionKeys"] } }),
            claimant_material: json!({ "AbpV1_1": { "DevAddr": "000000b1", "FCntStart": 7,
                "SessionKeys": provisioning_member("AbpV1_1")["SessionKeys"] } }),
            other_members: json!({}),
            same_key: true,
        },
        ConflictCase {
            case: "OTAA 1.0.x material whose AppKey differs",
            dev_eui: "00000000000000a3",
            holder_material: json!({ "OtaaV1_0_x": {
                "AppKey": "000000000000000000000000000000c1",
                "AppEui": "00000000000000c2",
            } }),
            claimant_material: json!({ "OtaaV1_0_x": {
                "AppKey": "000000000000000000000000000000c9",
                "AppEui": "00000000000000c2",
            } }),
            other_members: json!({ "Name": "second" }),
            same_key: false,
        },
        ConflictCase {
            case: "ABP 1.0.x material whose session keys differ",
            dev_eui: "00000000000000a4",
            holder_material: json!({ "AbpV1_0_x": { "DevAddr": "000000d1", "SessionKeys": {
                "NwkSKey": "000000000000000000000000000000d2",
                "AppSKey": "000000000000000000000000000000d3",
            } } }),
            claimant_material: json!({ "AbpV1_0_x": { "DevAddr": "000000d1", "SessionKeys": {
                "NwkSKey": "000000000000000000000000000000d2",
                "AppSKey": "000000000000000000000000000000d9",
            } } }),
            other_members: json!({}),
            same_key: false,
        },
        ConflictCase {
            case: "a claimant presenting no material where the registered device presents \
                   OTAA 1.1",
            dev_eui: "00000000000000a5",
            holder_material: json!({ "OtaaV1_1": provisioning_member("OtaaV1_1") }),
            claimant_material: json!({}),
            other_members: json!({}),
            same_key: false,
        },
        ConflictCase {
            case: "a claimant presenting ABP 1.0.x material where the registered device \
                   presents none",
            dev_eui: "00000000000000a6",
            holder_material: json!({}),
            claimant_material: json!({ "AbpV1_0_x": provisioning_member("AbpV1_0_x") }),
            other_members: json!({}),
            same_key: false,
        },
        ConflictCase {
            case: "the same OTAA 1.0.x material beside a different ABP block",
            dev_eui: "00000000000000a7",
            holder_material: json!({ "OtaaV1_0_x": provisioning_member("OtaaV1_0_x") }),
            claimant_material: json!({
                "OtaaV1_0_x": provisioning_member("OtaaV1_0_x"),
                "AbpV1_0_x": provisioning_member("AbpV1_0_x"),
            }),
            other_members: json!({}),
            same_key: false,
        },
    ]
}

/// A create that collides on a `DevEui` is refused with the sentence of the collision it is —
/// the already-claimed sentence when it re-presents the join key material the registered
/// device was registered with, the duplicate-identifier sentence when that material differs —
/// and neither sentence nor the body around it discloses anything about the device that holds
/// the identifier.
#[tokio::test]
async fn a_dev_eui_conflict_carries_the_sentence_of_the_collision_it_is() {
    let router = router();
    seed_destination(&router, "ConflictDestA").await;
    seed_destination(&router, "ConflictDestB").await;

    for case in conflict_cases() {
        let ConflictCase {
            case,
            dev_eui,
            holder_material,
            claimant_material,
            other_members,
            same_key,
        } = case;

        let mut holder_lorawan = json!({ "DevEui": dev_eui });
        for (key, value) in holder_material
            .as_object()
            .expect("the holder's key material is an object")
        {
            holder_lorawan[key.as_str()] = value.clone();
        }
        let (status, first) = create_device(
            &router,
            json!({
                "Type": "LoRaWAN",
                "Name": "first",
                "DestinationName": "ConflictDestA",
                "LoRaWAN": holder_lorawan,
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: the first create must be 201, got {status} {first}"
        );
        let owner = first["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {first}"))
            .to_string();

        let mut claimant_lorawan = json!({ "DevEui": dev_eui });
        for (key, value) in claimant_material
            .as_object()
            .expect("the claimant's key material is an object")
        {
            claimant_lorawan[key.as_str()] = value.clone();
        }
        let mut body = json!({
            "Type": "LoRaWAN",
            "DestinationName": "ConflictDestA",
            "LoRaWAN": claimant_lorawan,
        });
        for (key, value) in other_members
            .as_object()
            .expect("the other members are an object")
        {
            body[key.as_str()] = value.clone();
        }
        let (status, response) = create_device(&router, body).await;

        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: expected 409, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );
        let expected_conflict = if same_key {
            claimed_dev_eui_message(dev_eui)
        } else {
            DUPLICATE_DEV_EUI_MESSAGE.to_string()
        };
        assert_eq!(
            response["Message"],
            json!(expected_conflict),
            "case {case:?}: expected Message {expected_conflict:?}, got {response}"
        );
        let message = response["Message"].as_str().unwrap_or_default().to_string();
        if !same_key {
            // Two distinct devices collided, so the sentence is fixed text that names
            // neither of them.
            assert!(
                !message.contains(dev_eui),
                "case {case:?}: the refusal must not name the submitted identifier, got \
                 {message:?}"
            );
        }
        // Whichever sentence it is, nothing about the device that holds the identifier is
        // disclosed: not its id in the text, and not in `ResourceId`/`ResourceType`, which
        // this refusal leaves unset.
        assert!(
            !message.contains(&owner),
            "case {case:?}: the refusal must not name the identifier's owner, got {message:?}"
        );
        assert!(
            !response.to_string().contains(&owner),
            "case {case:?}: the refusal must disclose nothing about the owning device, got \
             {response}"
        );
        assert_eq!(
            response["ResourceId"],
            Value::Null,
            "case {case:?}: ResourceId must be unset, got {response}"
        );
        assert_eq!(
            response["ResourceType"],
            Value::Null,
            "case {case:?}: ResourceType must be unset, got {response}"
        );

        // The identifier still resolves to the device that claimed it first.
        let (status, resolved) = get_by_dev_eui(&router, dev_eui).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "case {case:?}: the identifier must still resolve, got {status} {resolved}"
        );
        assert_eq!(
            resolved["Id"],
            json!(owner),
            "case {case:?}: expected the first device {owner:?}, got {:?}",
            resolved["Id"]
        );
    }
}

/// `CreateWirelessDevice` raises `ConflictException` for three distinct cases — a `DevEui`
/// another device holds, the same `DevEui` re-presented with that device's own key material,
/// and a replayed `ClientRequestToken` used with different parameters — so each carries its own
/// sentence and a client reading the text can tell them apart.
#[tokio::test]
async fn the_conflict_classes_of_a_device_create_carry_their_own_sentences() {
    let router = router();
    seed_destination(&router, "TwoConflictsDest").await;

    // One registered device, then one create per case: the cases differ only in which
    // precondition the following create breaks.
    let dev_eui = "00000000000000b7";
    let material = provisioning_member("OtaaV1_0_x");
    let (status, first) = create_device(
        &router,
        json!({
            "Type": "LoRaWAN",
            "Name": "token-holder",
            "DestinationName": "TwoConflictsDest",
            "ClientRequestToken": "conflict-class-token",
            "LoRaWAN": { "DevEui": dev_eui, "OtaaV1_0_x": material },
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the first create must be 201, got {status} {first}"
    );

    let (status, duplicate) = create_device(
        &router,
        json!({
            "Type": "LoRaWAN",
            "Name": "dev-eui-claimant",
            "DestinationName": "TwoConflictsDest",
            "LoRaWAN": { "DevEui": dev_eui },
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the duplicate DevEui must be 409, got {status} {duplicate}"
    );
    let expected_conflict = DUPLICATE_DEV_EUI_MESSAGE;
    assert_eq!(
        duplicate["Message"],
        json!(expected_conflict),
        "expected Message {expected_conflict:?}, got {duplicate}"
    );

    let (status, reclaimed) = create_device(
        &router,
        json!({
            "Type": "LoRaWAN",
            "Name": "same-key-claimant",
            "DestinationName": "TwoConflictsDest",
            "LoRaWAN": { "DevEui": dev_eui, "OtaaV1_0_x": provisioning_member("OtaaV1_0_x") },
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "re-presenting the registered key material must be 409, got {status} {reclaimed}"
    );
    let expected_claimed = claimed_dev_eui_message(dev_eui);
    assert_eq!(
        reclaimed["Message"],
        json!(expected_claimed),
        "expected Message {expected_claimed:?}, got {reclaimed}"
    );

    let (status, replayed) = create_device(
        &router,
        json!({
            "Type": "LoRaWAN",
            "Name": "a-different-device",
            "DestinationName": "TwoConflictsDest",
            "ClientRequestToken": "conflict-class-token",
            "LoRaWAN": { "DevEui": "00000000000000b8" },
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "the replayed token must be 409, got {status} {replayed}"
    );
    assert_eq!(
        error_code(&replayed),
        "ConflictException",
        "expected ConflictException for the replayed token, got {replayed}"
    );
    let token_message = replayed["Message"].as_str().unwrap_or_default();
    assert_ne!(
        token_message, expected_conflict,
        "the token conflict must not carry the duplicate-DevEui sentence, got \
         {token_message:?}"
    );
    assert_ne!(
        token_message,
        expected_claimed.as_str(),
        "the token conflict must not carry the already-claimed sentence, got {token_message:?}"
    );
}

/// The already-claimed sentence is the class's fixed text around the identifier THAT request
/// submitted, for every identifier the model admits: a client can strip the fixed halves and
/// recover the colliding `DevEui`, and two different identifiers never answer the same text.
#[tokio::test]
async fn the_already_claimed_sentence_names_the_identifier_the_request_submitted() {
    let router = router();
    seed_destination(&router, "SubmittedEuiDest").await;

    let mut messages: Vec<String> = Vec::new();
    for (case, dev_eui) in DEV_EUI_CASES {
        // The two creates are identical, so the second re-presents the registered device's
        // own join key material and the refusal is the already-claimed one.
        let body = json!({
            "Type": "LoRaWAN",
            "Name": "first",
            "DestinationName": "SubmittedEuiDest",
            "LoRaWAN": { "DevEui": dev_eui, "OtaaV1_0_x": provisioning_member("OtaaV1_0_x") },
        });
        let (status, first) = create_device(&router, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "case {case:?}: the first create must be 201, got {status} {first}"
        );
        let owner = first["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {first}"))
            .to_string();

        let (status, response) = create_device(&router, body).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "case {case:?}: expected 409, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response}"
        );
        let expected = claimed_dev_eui_message(dev_eui);
        assert_eq!(
            response["Message"],
            json!(expected),
            "case {case:?}: expected Message {expected:?}, got {response}"
        );

        // Stripping the two fixed halves leaves exactly what the request submitted,
        // which is what a client reporting the collision or retrying with another
        // identifier reads out of the text.
        let message = response["Message"].as_str().unwrap_or_default().to_string();
        let recovered = message
            .strip_prefix(CLAIMED_DEV_EUI_PREFIX)
            .and_then(|rest| rest.strip_suffix(CLAIMED_DEV_EUI_SUFFIX));
        assert_eq!(
            recovered,
            Some(*dev_eui),
            "case {case:?}: the submitted identifier must be recoverable from {message:?}, \
             got {recovered:?}"
        );
        assert!(
            !response.to_string().contains(&owner),
            "case {case:?}: the refusal must disclose nothing about the owning device, got \
             {response}"
        );
        messages.push(message);
    }

    // The identifier is the sentence's VARIABLE part, so no two rows answered the same
    // text: a sentence with no variable part would collapse them all into one.
    let mut distinct = messages.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        messages.len(),
        "each submitted identifier must get its own message, got {messages:?}"
    );
}
