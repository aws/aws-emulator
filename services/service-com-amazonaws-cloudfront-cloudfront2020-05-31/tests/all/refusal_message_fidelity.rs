// SPDX-License-Identifier: CC0-1.0

//! What a REFUSAL of this service says, end to end over HTTP.
//!
//! One rule, stated three ways by the surfaces below: a refusal carries the service's own
//! fixed sentence for the condition it reports, and nothing the request submitted reaches
//! that sentence —
//!
//! * a PARAMETER outside the value set or the range CloudFront publishes for it is refused
//!   with the sentence that states the RULE (the parameter's name and the values or bounds it
//!   admits), never with the value that was sent: `CreateAnycastIpList`'s `IpCount` and the
//!   `SamplingRate` every real-time log configuration carries;
//! * an IDENTIFIER the operation cannot resolve is refused with the sentence for the
//!   CONDITION the store puts it in, as wide as the addressed surface reads its identifier:
//!   the public key reads apply NO format gate — their `Id` is a plain `string` the model
//!   constrains in no way, so every value the store holds no key for, of any shape, is the
//!   kind's does-not-exist sentence;
//! * where one operation's model serves no not-found code of its own, the identifier the
//!   store holds nothing for is still reported as ABSENT under the code the model does serve
//!   — `ListDistributionsByRealtimeLogConfig` answers `InvalidArgument` carrying the sentence
//!   a read of the same configuration answers with.
//!
//! The rows span what each rule generalises over: for `IpCount`, the empty allocation, one
//! below and one above each documented value, a negative count and the largest count there
//! could be; for `SamplingRate`, each published bound, one past each, and values far outside
//! on both sides, over the create AND the update that submit one, beside the two lists a
//! configuration has to name something in; for the public key identifier, the kind's own
//! namespace at a body too short and too long for it, each other namespace this service mints
//! ids in (a distribution's, a key group's UUID, both field-level encryption namespaces), and
//! identifiers CloudFront could have issued (one never minted, one character off the stored
//! one, the stored id in lower case, the caller reference, a bare digit, a word of no id shape
//! at all) — one condition, under one sentence — over every operation that addresses a public
//! key by `Id`; and for the listing, the
//! configuration named by `Name`, by `ARN` and by both.
//!
//! Each refusal is also shown to change nothing: the resource it named is read back
//! afterwards, and the name or rate a refused create asked for is still free.
//!
//! The wire harness (the router, the request helpers and the element scanner) is the one
//! [`crate::distribution_lifecycle`] builds, and the creates are the sibling suites'.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ANYCAST_IP_COUNTS, FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX,
    FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX, MAX_SAMPLING_RATE, MIN_SAMPLING_RATE,
    PUBLIC_KEY_ID_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, Create};
use crate::distribution_lifecycle::{
    call, config, emulator, is_distribution_id, is_minted_uuid, Answer, API_VERSION,
};
use crate::key_group_lifecycle::{create_group, Group};
use crate::public_key_delete::delete_key;
use crate::public_key_lifecycle::{create_key, get_key, pem, Key};
use crate::realtime_log_config_lifecycle::{get_config, strings, Endpoint, Submitted};
use crate::response_headers_policy_lifecycle::escape;
use crate::web_acl_association::create as create_distribution_returning_id;

/// The sentence `NoSuchPublicKey` carries for an `Id` the store holds no public key for,
/// spelled out here as a literal rather than read off the crate's producer, so the WORDING is
/// what these rows are held to.
const NO_SUCH_PUBLIC_KEY: &str = "The specified CloudFront public key does not exist.";

/// The sentence the same code carries for an `Id` outside the shape a public key id takes:
/// the identifier the service cannot read, named by the member that carried it.
const PUBLIC_KEY_ID_FORMAT_INVALID: &str = "Id format is invalid";

/// The sentence an `IpCount` outside the documented value set is refused with: it names the
/// parameter the quantity arrives under and the two counts a list may be allocated for.
const INVALID_IP_COUNT: &str = "The parameter Quantity Invalid quantity provided. The quantity \
                                of IPs in an AnycastIpList should be either 3 or 21.";

/// The sentence a `SamplingRate` outside the published range is refused with: the parameter
/// and the two ends of its range, worded with the lower end exclusive.
const SAMPLING_RATE_OUT_OF_RANGE: &str =
    "The parameter SamplingRate must be an integer greater than 0 and less than or equal to 100.";

/// The sentence every operation that cannot resolve a real-time log configuration reports it
/// absent with, whichever code its own model makes that sentence travel under.
const NO_SUCH_REALTIME_LOG_CONFIG: &str = "The specified real-time log configuration does not exist.";

// ---------------------------------------------------------------------------
// Calling the surfaces under test
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/public-key/{Id}/config` — the second reader of the one stored record.
async fn get_key_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/public-key/{Id}/config` with the `PublicKeyConfig` document `body`, under
/// the version token `if_match`.
async fn update_key(router: &MultiServiceRouter, id: &str, body: &str, if_match: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/public-key/{id}/config"),
        Some(body),
        Some(if_match),
    )
    .await
}

/// `POST /2020-05-31/realtime-log-config` with the request document `body`.
async fn create_realtime_log_config(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/realtime-log-config"),
        Some(body),
        None,
    )
    .await
}

/// `PUT /2020-05-31/realtime-log-config`, addressing the stored configuration by `name` and
/// submitting `sampling_rate` for it; the other parameters are left out, so the stored ones
/// stand.
async fn update_sampling_rate(
    router: &MultiServiceRouter,
    name: &str,
    sampling_rate: i64,
) -> Answer {
    let body = format!(
        "<UpdateRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Name>{}</Name>\
         <SamplingRate>{sampling_rate}</SamplingRate></UpdateRealtimeLogConfigRequest>",
        escape(name)
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/realtime-log-config"),
        Some(&body),
        None,
    )
    .await
}

/// `POST /2020-05-31/distributionsByRealtimeLogConfig`, addressing the configuration by
/// `name`, by `arn`, by both or by neither — each `None` leaves that element out entirely.
async fn list_by_config(
    router: &MultiServiceRouter,
    name: Option<&str>,
    arn: Option<&str>,
) -> Answer {
    let mut body = format!(
        "<ListDistributionsByRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(name) = name {
        body.push_str(&format!(
            "<RealtimeLogConfigName>{}</RealtimeLogConfigName>",
            escape(name)
        ));
    }
    if let Some(arn) = arn {
        body.push_str(&format!(
            "<RealtimeLogConfigArn>{}</RealtimeLogConfigArn>",
            escape(arn)
        ));
    }
    body.push_str("</ListDistributionsByRealtimeLogConfigRequest>");
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distributionsByRealtimeLogConfig"),
        Some(&body),
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// A parameter outside the values its member documents
// ---------------------------------------------------------------------------

/// `CreateAnycastIpListRequest$IpCount` is documented "Valid values: 21 or 3": a count outside
/// that set is refused with the sentence that states the rule, identical for every refused
/// count, and the refused create allocates nothing — the name it asked for is still free.
#[tokio::test]
async fn an_ip_count_outside_the_documented_values_states_the_rule() {
    let router = emulator();

    // The rows span the class: the empty allocation, one below and one above each documented
    // value, a negative count, and the largest count there could be.
    let refused: [(&str, i32); 8] = [
        ("no addresses at all", 0),
        ("one address", 1),
        ("one below the smaller documented value", 2),
        ("one above the smaller documented value", 4),
        ("one below the larger documented value", 20),
        ("one above the larger documented value", 22),
        ("a negative count", -3),
        ("every address there could be", i32::MAX),
    ];

    for (case, ip_count) in refused {
        let name = format!("ip-count-{}", ip_count.unsigned_abs());
        let create = Create {
            case: "an IpCount outside the documented values",
            name: name.clone(),
            ip_count,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_anycast_ip_list(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (IpCount {ip_count}): expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?} (IpCount {ip_count}): expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, INVALID_IP_COUNT,
            "case {case:?} (IpCount {ip_count}): expected {INVALID_IP_COUNT:?}, got {message:?}"
        );

        // The refused create allocated nothing and took no name: the same name under a
        // documented count is accepted.
        let admitted = Create {
            ip_count: ANYCAST_IP_COUNTS[0],
            ..create
        };
        let answer = create_anycast_ip_list(&router, &admitted.xml()).await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: the refused create must leave the name {name} free, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// `SamplingRate` is "a whole number between 1 and 100 (inclusive)": a rate outside that range
/// is refused with the sentence that states the rule — the same sentence whichever operation
/// submitted the rate — and the stored configuration is left exactly as it was. Each published
/// bound and a value between them is admitted.
#[tokio::test]
async fn a_sampling_rate_outside_the_published_range_states_the_rule() {
    let router = emulator();

    // One stored configuration the update rows run against, created at a rate inside the
    // range so every refusal below can be shown to leave it alone.
    let stored = Submitted {
        case: "the configuration every rate below is submitted against",
        name: "sampling-rate-rule".to_string(),
        sampling_rate: MIN_SAMPLING_RATE,
        end_points: vec![Endpoint::kinesis("sampling-rate-rule")],
        fields: strings(&["timestamp", "c-ip"]),
    };
    let created = create_realtime_log_config(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreateRealtimeLogConfig, got {} with {}",
        created.status, created.raw
    );

    // The rows span the class: one past each published bound on both sides, zero, a negative
    // rate and rates far outside the range.
    let refused: [(&str, i64); 6] = [
        ("one below the smallest rate", MIN_SAMPLING_RATE - 1),
        ("a negative rate", -1),
        ("one above the largest rate", MAX_SAMPLING_RATE + 1),
        ("a rate ten times the largest", MAX_SAMPLING_RATE * 10),
        ("a rate of a thousand", 1_000),
        ("the largest rate there could be", i64::MAX),
    ];

    for (case, rate) in refused {
        let submitted = Submitted {
            case: "a SamplingRate outside the published range",
            name: format!("sampling-rate-{}", rate.unsigned_abs()),
            sampling_rate: rate,
            end_points: vec![Endpoint::kinesis("refused")],
            fields: strings(&["timestamp"]),
        };
        for (operation, answer) in [
            (
                "CreateRealtimeLogConfig",
                create_realtime_log_config(&router, &submitted.xml()).await,
            ),
            (
                "UpdateRealtimeLogConfig",
                update_sampling_rate(&router, &stored.name, rate).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?} ({operation}, rate {rate}): expected HTTP 400, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case:?} ({operation}, rate {rate}): expected InvalidArgument, got {}",
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, SAMPLING_RATE_OUT_OF_RANGE,
                "case {case:?} ({operation}, rate {rate}): expected \
                 {SAMPLING_RATE_OUT_OF_RANGE:?}, got {message:?}"
            );
        }

        // Neither refusal stored anything: the name the create asked for still answers to no
        // configuration, and the stored one still carries the rate it was created with.
        let absent = get_config(&router, Some(&submitted.name), None).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?} (rate {rate}): the refused create must store nothing, got {} with {}",
            absent.status, absent.raw
        );
        let held = get_config(&router, Some(&stored.name), None).await;
        let rate_held = held.root().find_text("SamplingRate").unwrap_or_default();
        assert_eq!(
            rate_held,
            MIN_SAMPLING_RATE.to_string(),
            "case {case:?} (rate {rate}): the refused update must leave the stored rate at \
             {MIN_SAMPLING_RATE}, got {rate_held:?}"
        );
    }

    // Every rate inside the range is admitted: both published bounds and one between them.
    for rate in [MIN_SAMPLING_RATE, (MIN_SAMPLING_RATE + MAX_SAMPLING_RATE) / 2, MAX_SAMPLING_RATE] {
        let accepted = update_sampling_rate(&router, &stored.name, rate).await;
        assert_eq!(
            accepted.status, 200,
            "case an admitted rate {rate}: expected HTTP 200, got {} with {}",
            accepted.status, accepted.raw
        );
        let held = get_config(&router, Some(&stored.name), None).await;
        let rate_held = held.root().find_text("SamplingRate").unwrap_or_default();
        assert_eq!(
            rate_held,
            rate.to_string(),
            "case an admitted rate {rate}: expected the stored rate to move to it, got \
             {rate_held:?}"
        );
    }
}

/// A configuration "send[s] real-time log data to the specified Amazon Kinesis data stream" and
/// states "the fields that are included in each real-time log record", so each of those two
/// lists has to name something: a create that submits one of them EMPTY is refused with the
/// sentence that names the MEMBER and what one entry of it is, and stores nothing.
#[tokio::test]
async fn a_list_parameter_with_no_entries_names_the_member_and_its_entry() {
    let router = emulator();

    // One row per list a configuration has to name something in, each submitted empty while
    // the other carries its entries, so the sentence under test is the one for that member.
    let rows: [(&str, &str, Submitted); 2] = [
        (
            "no endpoints at all",
            "The parameter EndPoints must contain at least one endpoint.",
            Submitted {
                case: "a configuration that names no endpoint",
                name: "empty-end-points".to_string(),
                sampling_rate: MIN_SAMPLING_RATE,
                end_points: Vec::new(),
                fields: strings(&["timestamp", "c-ip"]),
            },
        ),
        (
            "no fields at all",
            "The parameter Fields must contain at least one field.",
            Submitted {
                case: "a configuration that includes no field",
                name: "empty-fields".to_string(),
                sampling_rate: MAX_SAMPLING_RATE,
                end_points: vec![Endpoint::kinesis("empty-fields")],
                fields: Vec::new(),
            },
        ),
    ];

    for (case, expected, submitted) in &rows {
        let answer = create_realtime_log_config(&router, &submitted.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, *expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The refused create stored nothing: the name it asked for answers to no
        // configuration.
        let absent = get_config(&router, Some(&submitted.name), None).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the refused create must store nothing, got {} with {}",
            absent.status, absent.raw
        );
    }
}

// ---------------------------------------------------------------------------
// An identifier of another namespace, against one that is merely absent
// ---------------------------------------------------------------------------

/// What a row expects of every operation that addresses a public key by `Id`.
#[derive(Debug, Clone)]
struct IdRow {
    /// What this row is a case of, named in every assertion it drives.
    case: &'static str,
    /// The `Id` the request carries.
    id: String,
    /// The sentence the refusal has to carry.
    message: &'static str,
}

/// Every operation that addresses a public key by `Id` answers `NoSuchPublicKey` (404) for an
/// `Id` it cannot resolve, and the SENTENCE splits the two ways that happens: only a value
/// that is a well-formed identifier of ANOTHER resource kind this service mints — a
/// distribution, a key group, a field-level encryption profile or configuration — could never
/// have been a public key id, so it is the identifier the service cannot READ; every other
/// value the store holds no key for — an id of the public key namespace that was never minted
/// or is of no minted id's width, a name, a caller reference, a word of no id shape at all —
/// is the key that is ABSENT. No refusal touches the key the account really holds.
#[tokio::test]
async fn every_id_the_store_holds_no_public_key_for_is_reported_absent() {
    let router = emulator();

    // The one public key the account holds, and the two other namespaces whose ids are MINTED
    // here rather than written by hand: a distribution's and a key group's.
    let present = Key {
        case: "the one key the account holds".to_string(),
        caller_reference: "refusal-message-fidelity".to_string(),
        name: "refusal-message-fidelity".to_string(),
        encoded_key: pem(2, 'R'),
        comment: Some("the key every refusal below must leave alone".to_string()),
    };
    let created = create_key(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreatePublicKey, got {} with {}",
        created.status, created.raw
    );
    let known = created.root().text_at("Id").unwrap_or_default().to_string();
    let etag = created.header("etag").to_string();

    let (distribution_id, _) = create_distribution_returning_id(
        &router,
        "a distribution whose id is of another namespace",
        &config("refusal-message-fidelity", "refusal-message-fidelity").xml(),
    )
    .await;
    assert!(
        is_distribution_id(&distribution_id),
        "expected a minted distribution id, got {distribution_id:?}"
    );
    let group = Group {
        case: "a key group whose id is a UUID".to_string(),
        name: "refusal-message-fidelity".to_string(),
        items: vec![known.clone()],
        comment: None,
    };
    let group_answer = create_group(&router, &group.xml()).await;
    assert_eq!(
        group_answer.status, 201,
        "expected HTTP 201 from CreateKeyGroup, got {} with {}",
        group_answer.status, group_answer.raw
    );
    let group_id = group_answer
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert!(
        is_minted_uuid(&group_id),
        "expected a key group id drawn as a UUID, got {group_id:?}"
    );

    let rows: Vec<IdRow> = vec![
        IdRow {
            case: "a distribution id where a public key id belongs",
            id: distribution_id,
            message: PUBLIC_KEY_ID_FORMAT_INVALID,
        },
        IdRow {
            case: "a key group id, which this service draws as a UUID",
            id: group_id,
            message: PUBLIC_KEY_ID_FORMAT_INVALID,
        },
        IdRow {
            // The prefix is the one this service mints profile ids under
            // (`FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX`), followed by a body of the id
            // alphabet, as `P280MFCLSYOCVU` is.
            case: "a field-level encryption profile id",
            id: format!("{FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX}2AAAAAAAAAAAA"),
            message: PUBLIC_KEY_ID_FORMAT_INVALID,
        },
        IdRow {
            // The sibling field-level encryption CONFIG namespace
            // (`FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX`).
            case: "a field-level encryption configuration id",
            id: format!("{FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX}3AAAAAAAAAAAA"),
            message: PUBLIC_KEY_ID_FORMAT_INVALID,
        },
        IdRow {
            // In the public key namespace (`PUBLIC_KEY_ID_PREFIX`) though of no minted id's
            // width: the namespace is what this read decides on, so it is read as a public
            // key id and answered as the key that is not there.
            case: "the public key prefix with too short a body",
            id: format!("{PUBLIC_KEY_ID_PREFIX}2AAA"),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "the public key prefix with too long a body",
            id: format!("{PUBLIC_KEY_ID_PREFIX}2AAAAAAAAAAAAAAAAAAA"),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "the stored id with one character appended",
            id: format!("{known}9"),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "a public key id of this namespace that was never minted",
            id: "K2NEVERMINTED1".to_string(),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "the stored id with its last character changed",
            id: {
                let mut body: Vec<char> = known.chars().collect();
                let last = body.len() - 1;
                body[last] = if body[last] == 'A' { 'B' } else { 'A' };
                body.into_iter().collect()
            },
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "the stored id in lower case",
            id: known.to_lowercase(),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "the caller reference rather than the id",
            id: present.caller_reference.clone(),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "a bare digit",
            id: "0".to_string(),
            message: NO_SUCH_PUBLIC_KEY,
        },
        IdRow {
            case: "a word of no CloudFront id shape at all",
            id: "not-a-public-key-id".to_string(),
            message: NO_SUCH_PUBLIC_KEY,
        },
    ];

    let body = Key {
        comment: Some("never stored".to_string()),
        ..present.clone()
    }
    .xml();

    for row in &rows {
        for (operation, answer) in [
            ("GetPublicKey", get_key(&router, &row.id).await),
            ("GetPublicKeyConfig", get_key_config(&router, &row.id).await),
            (
                "UpdatePublicKey",
                update_key(&router, &row.id, &body, &etag).await,
            ),
            (
                "DeletePublicKey",
                delete_key(&router, &row.id, Some(&etag)).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {:?} ({operation}, Id {:?}): expected HTTP 404, got {} with {}",
                row.case, row.id, answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchPublicKey",
                "case {:?} ({operation}, Id {:?}): expected NoSuchPublicKey, got {}",
                row.case,
                row.id,
                answer.raw
            );
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, row.message,
                "case {:?} ({operation}, Id {:?}): expected {:?}, got {message:?}",
                row.case, row.id, row.message
            );
            assert!(
                !message.contains(&row.id),
                "case {:?} ({operation}): the refusal must not echo the Id, got {message:?}",
                row.case
            );
        }
    }

    // None of those refusals touched the key the account really holds.
    let still = get_key(&router, &known).await;
    assert_eq!(
        still.status, 200,
        "expected the stored key to still answer, got {} with {}",
        still.status, still.raw
    );
    assert_eq!(
        still.header("etag"),
        etag,
        "expected the stored key to keep its version token, got {:?}",
        still.header("etag")
    );
    present.assert_matches(
        still
            .root()
            .at("PublicKeyConfig")
            .unwrap_or_else(|| panic!("no configuration in {}", still.raw)),
        "the key every refusal left alone",
        &still.raw,
    );
}

// ---------------------------------------------------------------------------
// An absent resource reported under the one code the model serves
// ---------------------------------------------------------------------------

/// `ListDistributionsByRealtimeLogConfig` models `InvalidArgument` alone, so an identifier no
/// stored configuration answers to travels under that code — carrying the sentence a read of
/// the same configuration answers with, whichever member the identifier arrived in. A
/// configuration that IS stored lists from the store instead.
#[tokio::test]
async fn a_listing_of_an_unknown_realtime_log_config_reports_it_absent() {
    let router = emulator();

    let stored = Submitted {
        case: "the one configuration the account holds",
        name: "listing-absent-config".to_string(),
        sampling_rate: MAX_SAMPLING_RATE,
        end_points: vec![Endpoint::kinesis("listing-absent-config")],
        fields: strings(&["timestamp"]),
    };
    let created = create_realtime_log_config(&router, &stored.xml()).await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 from CreateRealtimeLogConfig, got {} with {}",
        created.status, created.raw
    );
    let arn = created
        .root()
        .find_text("ARN")
        .unwrap_or_default()
        .to_string();

    // The rows span the ways an identifier can address no configuration: by name, by ARN, by
    // both at once, by a name that is the ARN of the stored one, and by the stored name under
    // another case (a name identifies ONE configuration exactly).
    let absent: Vec<(&str, Option<String>, Option<String>)> = vec![
        (
            "a name no configuration holds",
            Some("no-such-configuration".to_string()),
            None,
        ),
        (
            "an ARN no configuration holds",
            None,
            Some(format!("{arn}-absent")),
        ),
        (
            "both identifiers, neither held",
            Some("no-such-configuration".to_string()),
            Some(format!("{arn}-absent")),
        ),
        (
            "the stored ARN submitted as the name",
            Some(arn.clone()),
            None,
        ),
        (
            "the stored name in upper case",
            Some(stored.name.to_uppercase()),
            None,
        ),
    ];

    for (case, name, config_arn) in &absent {
        let answer = list_by_config(&router, name.as_deref(), config_arn.as_deref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case:?}: expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, NO_SUCH_REALTIME_LOG_CONFIG,
            "case {case:?}: expected {NO_SUCH_REALTIME_LOG_CONFIG:?}, got {message:?}"
        );
    }

    // The configuration that IS stored is served, by either identifier: the refusals above are
    // about the identifier, not about the operation.
    for (case, name, config_arn) in [
        ("by name", Some(stored.name.clone()), None),
        ("by ARN", None, Some(arn.clone())),
    ] {
        let answer = list_by_config(&router, name.as_deref(), config_arn.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 for the stored configuration, got {} with {}",
            answer.status, answer.raw
        );
    }
}
