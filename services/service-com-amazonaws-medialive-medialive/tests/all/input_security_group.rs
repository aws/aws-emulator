// SPDX-License-Identifier: CC0-1.0

//! The MediaLive input security group lifecycle, end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{MAX_WHITELIST_RULES, TRANSIENT_STATE_SPAN};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive, ACCOUNT};

/// Long enough for a transient channel state to have come due, with margin.
fn wait_for_the_clock() {
    std::thread::sleep(TRANSIENT_STATE_SPAN + Duration::from_millis(200));
}

/// The source URLs a PULL-type input ingests through — `Sources` is "The source
/// URLs for a PULL-type input", so an input of such a type carries them, and
/// `CreateInput` refuses one that carries none. This suite is about the security
/// GROUP, so every pull input it creates takes this same redundant pair.
fn pull_sources() -> Value {
    json!([
        { "url": "https://example.com/a.m3u8" },
        { "url": "https://example.com/b.m3u8" },
    ])
}

/// `CreateInputSecurityGroup`, asserting the 201 the recorded wire response
/// carries and returning the `SecurityGroup` it reports.
///
/// Shared with the `input_security_group_listing` suite, so the two cannot
/// disagree about how a group is created.
pub async fn create_group(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputSecurityGroups", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInputSecurityGroup: expected 201, got {status}: {response}"
    );
    response["securityGroup"].clone()
}

/// `DescribeInputSecurityGroup` on one group, as a client calls it. Shared with
/// the `input_security_group_listing` suite.
pub async fn describe_group(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputSecurityGroups/{id}"), None).await
}

/// `DeleteInputSecurityGroup` on one group, as a client calls it: DELETE on the
/// group's own path (`smithy_query(query="operation",
/// operation="DeleteInputSecurityGroup")`: DELETE
/// /prod/inputSecurityGroups/{InputSecurityGroupId}). Shared with the
/// `input_security_group_listing` suite.
pub async fn delete_group(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await
}

/// The `State` a group reports right now, read back through
/// `DescribeInputSecurityGroup`.
async fn state_of(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, described) = describe_group(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInputSecurityGroup({id}): expected 200, got {status}: {described}"
    );
    described["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `UpdateInputSecurityGroup` on one group, as a client calls it: PUT on the
/// group's own path (`smithy_query(query="operation",
/// operation="UpdateInputSecurityGroup")`: PUT /prod/inputSecurityGroups/{InputSecurityGroupId}).
pub async fn update_group(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/prod/inputSecurityGroups/{id}"),
        Some(body),
    )
    .await
}

/// The `whitelistRules` projection of a list of CIDRs, as the wire carries it
/// (`InputWhitelistRule$Cidr`, jsonName `cidr`).
///
/// Shared with the `post_delete_mutation_classification` suite, so the two
/// cannot disagree about how a whitelist reaches the wire.
pub fn whitelist(cidrs: &[&str]) -> Value {
    Value::Array(cidrs.iter().map(|cidr| json!({ "cidr": cidr })).collect())
}

/// Rule: `CreateInputSecurityGroup` persists the request's whitelist rules (in
/// order) and tags under a minted numeric `Id` with an ARN built from the
/// request's region and that id, and every later read of the group answers from
/// that stored record.
///
/// The table spans what the rule generalises over: how many rules the request
/// carries (none, one, several), the prefix lengths they use — including both
/// published IPv4 bounds, /0 and /32 — the order they arrive in, and whether
/// tags are present. Members from
/// `smithy_query(query="operation", operation="CreateInputSecurityGroup")`.
#[tokio::test]
async fn created_groups_are_read_back_through_describe() {
    // (case, request body, the CIDRs the group must report, in order)
    let table: [(&str, Value, &[&str]); 5] = [
        ("no rules at all", json!({}), &[]),
        (
            "one host route",
            json!({ "whitelistRules": [{ "cidr": "192.0.2.1/32" }] }),
            &["192.0.2.1/32"],
        ),
        (
            "both published prefix bounds",
            json!({ "whitelistRules": [{ "cidr": "0.0.0.0/0" }, { "cidr": "10.0.0.0/32" }] }),
            &["0.0.0.0/0", "10.0.0.0/32"],
        ),
        (
            "several rules keep their order",
            json!({
                "whitelistRules": [
                    { "cidr": "203.0.113.0/24" },
                    { "cidr": "198.51.100.128/25" },
                    { "cidr": "10.0.0.0/8" },
                ]
            }),
            &["203.0.113.0/24", "198.51.100.128/25", "10.0.0.0/8"],
        ),
        (
            "rules and tags",
            json!({
                "whitelistRules": [{ "cidr": "172.16.0.0/12" }],
                "tags": { "env": "test", "owner": "fixer" },
            }),
            &["172.16.0.0/12"],
        ),
    ];

    for (case, request, cidrs) in table {
        let router = medialive();
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let created = create_group(&router, case, request).await;

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric input security group id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!(
                "arn:aws:medialive:us-west-2:{ACCOUNT}:inputSecurityGroup:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["whitelistRules"],
            whitelist(cidrs),
            "case {case:?}: expected the request's rules in order, got {:?}",
            created["whitelistRules"]
        );
        assert_eq!(
            created["tags"], expected_tags,
            "case {case:?}: expected the request's tags, got {:?}",
            created["tags"]
        );
        // A brand-new group is used by nothing, so it is IDLE with no inputs and
        // no channels (`InputSecurityGroupState`: IDLE | IN_USE | UPDATING |
        // DELETED).
        assert_eq!(
            created["state"],
            json!("IDLE"),
            "case {case:?}: expected a new group to be IDLE, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["inputs"],
            json!([]),
            "case {case:?}: expected no inputs to use a new group, got {:?}",
            created["inputs"]
        );
        assert_eq!(
            created["channels"],
            json!([]),
            "case {case:?}: expected no channels to use a new group, got {:?}",
            created["channels"]
        );

        // Round trip: the reader answers from the stored record, so it reports
        // exactly what the create reported.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInputSecurityGroup: expected 200, got {status}: {described}"
        );
        for member in ["arn", "id", "inputs", "state", "tags", "whitelistRules", "channels"] {
            assert_eq!(
                described[member], created[member],
                "case {case:?}: expected describe to report the created {member}, got {:?}",
                described[member]
            );
        }
    }
}

/// Rule: `CreateInputSecurityGroup` has no idempotency token, so every create
/// mints its own group, and each one is readable on its own id while the others
/// keep their rules.
#[tokio::test]
async fn each_create_mints_its_own_group() {
    let router = medialive();
    // (case, the group's single CIDR)
    let table: [(&str, &str); 3] = [
        ("first", "10.1.0.0/16"),
        ("second", "10.2.0.0/16"),
        // Same rules as the first row: a repeated request is still a new group.
        ("repeat of the first", "10.1.0.0/16"),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, cidr) in table {
        let group = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": cidr }] }),
        )
        .await;
        let id = group["id"].as_str().unwrap_or_default().to_string();
        assert!(
            created.iter().all(|(seen, _)| seen != &id),
            "case {case:?}: expected a fresh id, got the already-minted {id:?}"
        );
        created.push((id, cidr.to_string()));
    }

    for (id, cidr) in &created {
        let (status, described) = describe_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected the group to still exist, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            whitelist(&[cidr.as_str()]),
            "case {id:?}: expected its own rule {cidr:?}, got {:?}",
            described["whitelistRules"]
        );
    }
}

/// Rule: a whitelist rule must name an IPv4 CIDR with a prefix length of 0 to 32
/// — the member is documented "List of IPv4 CIDR addresses to whitelist" — and a
/// request carrying anything else is refused with the operation's modelled
/// `BadRequestException` (400) instead of creating a group.
///
/// The table spans the ways a value can fall outside that rule: no prefix at
/// all, a prefix one past the published bound, a non-numeric or negative prefix,
/// an octet out of range, an IPv6 prefix, an empty string and a rule that names
/// no CIDR. The valid bounds (/0 and /32) are a row of the create suite above.
#[tokio::test]
async fn whitelist_rules_outside_the_ipv4_cidr_rule_are_refused() {
    // (case, the whitelistRules the request carries)
    let table: [(&str, Value); 9] = [
        ("bare address, no prefix", json!([{ "cidr": "10.0.0.0" }])),
        ("prefix one past the bound", json!([{ "cidr": "10.0.0.0/33" }])),
        ("negative prefix", json!([{ "cidr": "10.0.0.0/-1" }])),
        ("non-numeric prefix", json!([{ "cidr": "10.0.0.0/x" }])),
        ("octet out of range", json!([{ "cidr": "256.0.0.0/8" }])),
        ("too few octets", json!([{ "cidr": "10.0.0/8" }])),
        ("an IPv6 prefix", json!([{ "cidr": "2001:db8::/32" }])),
        ("empty string", json!([{ "cidr": "" }])),
        ("a rule that names no CIDR", json!([{}])),
    ];

    for (case, rules) in table {
        let router = medialive();
        let (status, body) = call(
            &router,
            "POST",
            "/prod/inputSecurityGroups",
            Some(json!({ "whitelistRules": rules })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {body}"
        );

        // A refused request persists no group and leaves the operation working:
        // the create that follows it succeeds and is readable.
        let created = create_group(&router, case, json!({})).await;
        let (status, described) = describe_group(
            &router,
            created["id"].as_str().unwrap_or_default(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the following create to persist a group, got {status}: {described}"
        );
    }
}

/// Rule: `WhitelistRules` is a caller-chosen list this emulator KEEPS on the
/// stored group, so its length is bounded exactly as a network's CIDR lists are:
/// a request at or below the ceiling is accepted and stores every rule it
/// carried, and one entry past it is refused with the modelled
/// `BadRequestException` (400) with nothing written — for the create and for the
/// update alike, since the two share one validator.
///
/// The rows span the published bound from both sides (one rule, the ceiling, one
/// past it), because that is what a bound generalises over.
#[tokio::test]
async fn the_whitelist_of_one_group_is_bounded() {
    // (case, how many rules the request carries, whether it is accepted)
    let table: [(&str, usize, bool); 3] = [
        ("one rule", 1, true),
        ("at the ceiling", MAX_WHITELIST_RULES, true),
        ("one past the ceiling", MAX_WHITELIST_RULES + 1, false),
    ];

    for (case, count, accepted) in table {
        let cidrs: Vec<String> = (0..count)
            .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
            .collect();
        let rules: Vec<Value> = cidrs.iter().map(|cidr| json!({ "cidr": cidr })).collect();
        let expected = whitelist(
            &cidrs.iter().map(String::as_str).collect::<Vec<&str>>(),
        );

        // The create, the first writer of a group's whitelist.
        let router = medialive();
        let (status, body) = call(
            &router,
            "POST",
            "/prod/inputSecurityGroups",
            Some(json!({ "whitelistRules": rules.clone() })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected {count} rules to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["securityGroup"]["whitelistRules"], expected,
                "case {case:?}: expected the accepted rules to be stored, got {:?}",
                body["securityGroup"]["whitelistRules"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected {count} rules to be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {body}"
            );
            let (status, listed) = call(&router, "GET", "/prod/inputSecurityGroups", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListInputSecurityGroups: got {status}: {listed}"
            );
            assert_eq!(
                listed["inputSecurityGroups"],
                json!([]),
                "case {case:?}: expected the refused create to store no group, got {:?}",
                listed["inputSecurityGroups"]
            );
        }

        // The update, which rewrites the same list under the same ceiling: a
        // refused one leaves the group with the whitelist it already had.
        let router = medialive();
        let created = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": "192.0.2.0/24" }] }),
        )
        .await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let (status, body) = update_group(&router, &id, json!({ "whitelistRules": rules })).await;
        let (_, described) = describe_group(&router, &id).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update of {count} rules to be accepted, got {status}: {body}"
            );
            assert_eq!(
                described["whitelistRules"], expected,
                "case {case:?}: expected the updated rules to be stored, got {:?}",
                described["whitelistRules"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update of {count} rules to be refused, got {status}: {body}"
            );
            assert_eq!(
                described["whitelistRules"],
                whitelist(&["192.0.2.0/24"]),
                "case {case:?}: expected the refused update to leave the whitelist alone, got {:?}",
                described["whitelistRules"]
            );
        }
    }
}

/// Rule: a group's `State`, `Inputs` and `Channels` report what currently uses
/// it, so a group is `IN_USE` exactly while some resource in state references it
/// and `IDLE` otherwise — and the reader tracks every later create and delete
/// rather than freezing what the create saw.
#[tokio::test]
async fn a_group_is_in_use_exactly_while_a_resource_references_it() {
    let router = medialive();
    let attached = create_group(
        &router,
        "attached",
        json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
    )
    .await;
    let attached_id = attached["id"].as_str().unwrap_or_default().to_string();
    let untouched = create_group(&router, "untouched", json!({})).await;
    let untouched_id = untouched["id"].as_str().unwrap_or_default().to_string();

    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "uses-the-group",
            "channelSecurityGroups": [attached_id.clone()],
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (_, described) = describe_group(&router, &attached_id).await;
    assert_eq!(
        described["state"],
        json!("IN_USE"),
        "case \"a channel attached\": expected IN_USE, got {:?}",
        described["state"]
    );
    assert_eq!(
        described["channels"],
        json!([channel_id.clone()]),
        "case \"a channel attached\": expected the attaching channel's id, got {:?}",
        described["channels"]
    );

    let (_, other) = describe_group(&router, &untouched_id).await;
    assert_eq!(
        other["state"],
        json!("IDLE"),
        "case \"no resource attached\": expected IDLE, got {:?}",
        other["state"]
    );
    assert_eq!(
        other["channels"],
        json!([]),
        "case \"no resource attached\": expected no channels, got {:?}",
        other["channels"]
    );
    assert_eq!(
        other["inputs"],
        json!([]),
        "case \"no resource attached\": expected no inputs, got {:?}",
        other["inputs"]
    );

    // The other half of the same rule: an INPUT that names the group puts it
    // IN_USE too, and the group reports that input's id.
    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": "uses-the-other-group",
            "type": "URL_PULL",
            "sources": pull_sources(),
            "inputSecurityGroups": [untouched_id.clone()],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let input_id = created_input["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, attached_by_input) = describe_group(&router, &untouched_id).await;
    assert_eq!(
        attached_by_input["state"],
        json!("IN_USE"),
        "case \"an input attached\": expected IN_USE, got {:?}",
        attached_by_input["state"]
    );
    assert_eq!(
        attached_by_input["inputs"],
        json!([input_id]),
        "case \"an input attached\": expected the attaching input's id, got {:?}",
        attached_by_input["inputs"]
    );

    // Deleting the channel releases the group: once the delete has come due the
    // group is used by nothing again.
    wait_for_the_clock();
    let (status, deleting) = call(
        &router,
        "DELETE",
        &format!("/prod/channels/{channel_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteChannel: expected 200, got {status}: {deleting}");
    wait_for_the_clock();

    let (_, released) = describe_group(&router, &attached_id).await;
    assert_eq!(
        released["state"],
        json!("IDLE"),
        "case \"the attached channel deleted\": expected IDLE again, got {:?}",
        released["state"]
    );
    assert_eq!(
        released["channels"],
        json!([]),
        "case \"the attached channel deleted\": expected no channels, got {:?}",
        released["channels"]
    );
}

/// Rule: `DescribeInputSecurityGroup` on an id no group has answers the
/// operation's modelled `NotFoundException` (404).
///
/// The table spans the kinds of id that name no group, including a channel id —
/// the two id spaces are separate, so a channel's id is not a group's.
#[tokio::test]
async fn describe_refuses_an_id_no_group_has() {
    let router = medialive();
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "not-a-security-group",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // One group exists, so the store is not simply empty.
    create_group(&router, "a real group", json!({})).await;

    // (case, the id to read)
    let table: [(&str, String); 4] = [
        ("zero", "0".to_string()),
        ("a numeric id never minted", "9999999".to_string()),
        ("a non-numeric id", "not-an-id".to_string()),
        ("a channel's id", channel_id),
    ];

    for (case, id) in table {
        let (status, body) = describe_group(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

/// Rule: the input security group store is bounded, so a create past the ceiling
/// answers the operation's modelled throttle rather than growing the emulator's
/// heap, and every group created up to the ceiling is still readable.
#[tokio::test]
async fn the_group_store_is_bounded() {
    use service_com_amazonaws_medialive_medialive::state::MAX_INPUT_SECURITY_GROUPS;

    let router = medialive();
    let mut ids = Vec::new();
    for index in 0..MAX_INPUT_SECURITY_GROUPS {
        let group = create_group(
            &router,
            "filling the store",
            json!({ "whitelistRules": [{ "cidr": format!("10.0.{}.0/24", index % 256) }] }),
        )
        .await;
        ids.push(group["id"].as_str().unwrap_or_default().to_string());
    }

    let (status, body) = call(
        &router,
        "POST",
        "/prod/inputSecurityGroups",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 429,
        "case {MAX_INPUT_SECURITY_GROUPS} stored: expected the create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_INPUT_SECURITY_GROUPS} stored: expected TooManyRequestsException, got {body}"
    );

    for (index, id) in ids.iter().enumerate() {
        let (status, described) = describe_group(&router, id).await;
        assert_eq!(
            status, 200,
            "case group {index}: expected it to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            whitelist(&[format!("10.0.{}.0/24", index % 256).as_str()]),
            "case group {index}: expected its own rule, got {:?}",
            described["whitelistRules"]
        );
    }
}

/// Rule: `UpdateInputSecurityGroup` writes the members the request carries onto
/// the stored group and leaves the members it omits as they were, so every later
/// read of the group answers from the updated record — the group keeps its id and
/// its ARN, and the update's own response is that same record.
///
/// The table spans what the rule generalises over: which member the request
/// carries (whitelist only, tags only, both, neither), how many rules replace how
/// many (one for several, several for one, both published prefix bounds), the
/// order they arrive in, and the empty list that clears the whitelist — which is
/// a VALUE, not an omission.
#[tokio::test]
async fn an_update_writes_what_it_carries_and_leaves_the_rest() {
    // (case, the group's starting rules and tags, the update body,
    //  the CIDRs it must report after, the tags it must report after)
    let table: [(&str, Value, Value, &[&str], Value); 9] = [
        (
            "an update carrying no members at all leaves the group as it was",
            json!({
                "whitelistRules": [{ "cidr": "10.8.0.0/16" }],
                "tags": { "env": "test" },
            }),
            json!({}),
            &["10.8.0.0/16"],
            json!({ "env": "test" }),
        ),
        (
            "one rule replaced by another",
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
            json!({ "whitelistRules": [{ "cidr": "192.0.2.0/24" }] }),
            &["192.0.2.0/24"],
            json!({}),
        ),
        (
            "one rule replaced by several, in order",
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
            json!({
                "whitelistRules": [
                    { "cidr": "203.0.113.0/24" },
                    { "cidr": "198.51.100.128/25" },
                    { "cidr": "172.16.0.0/12" },
                ]
            }),
            &["203.0.113.0/24", "198.51.100.128/25", "172.16.0.0/12"],
            json!({}),
        ),
        (
            "several rules replaced by one",
            json!({
                "whitelistRules": [{ "cidr": "10.1.0.0/16" }, { "cidr": "10.2.0.0/16" }]
            }),
            json!({ "whitelistRules": [{ "cidr": "10.3.0.0/16" }] }),
            &["10.3.0.0/16"],
            json!({}),
        ),
        (
            "both published prefix bounds",
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
            json!({ "whitelistRules": [{ "cidr": "0.0.0.0/0" }, { "cidr": "198.51.100.7/32" }] }),
            &["0.0.0.0/0", "198.51.100.7/32"],
            json!({}),
        ),
        (
            "an empty list clears the whitelist",
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
            json!({ "whitelistRules": [] }),
            &[],
            json!({}),
        ),
        (
            "a group that had no rules gets some",
            json!({}),
            json!({ "whitelistRules": [{ "cidr": "10.4.0.0/16" }] }),
            &["10.4.0.0/16"],
            json!({}),
        ),
        (
            "tags only: the whitelist is left alone",
            json!({
                "whitelistRules": [{ "cidr": "10.5.0.0/16" }],
                "tags": { "env": "test" },
            }),
            json!({ "tags": { "env": "prod", "owner": "fixer" } }),
            &["10.5.0.0/16"],
            json!({ "env": "prod", "owner": "fixer" }),
        ),
        (
            "both members at once",
            json!({
                "whitelistRules": [{ "cidr": "10.6.0.0/16" }],
                "tags": { "env": "test" },
            }),
            json!({
                "whitelistRules": [{ "cidr": "10.7.0.0/16" }],
                "tags": { "env": "staging" },
            }),
            &["10.7.0.0/16"],
            json!({ "env": "staging" }),
        ),
    ];

    for (case, created_with, update, cidrs, expected_tags) in table {
        let router = medialive();
        let created = create_group(&router, case, created_with).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, updated) = update_group(&router, &id, update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInputSecurityGroup: expected 200, got {status}: {updated}"
        );
        let group = updated["securityGroup"].clone();
        assert_eq!(
            group["whitelistRules"],
            whitelist(cidrs),
            "case {case:?}: expected the group's rules to be {cidrs:?}, got {:?}",
            group["whitelistRules"]
        );
        assert_eq!(
            group["tags"], expected_tags,
            "case {case:?}: expected the group's tags to be {expected_tags}, got {:?}",
            group["tags"]
        );
        // An update changes the group, never its identity.
        assert_eq!(
            group["id"], created["id"],
            "case {case:?}: expected the group to keep its id {:?}, got {:?}",
            created["id"], group["id"]
        );
        assert_eq!(
            group["arn"], created["arn"],
            "case {case:?}: expected the group to keep its ARN {:?}, got {:?}",
            created["arn"], group["arn"]
        );
        // Nothing uses these groups, so the update leaves them IDLE with no
        // inputs and no channels: `State` reports what uses the group, which a
        // whitelist change does not touch.
        assert_eq!(
            group["state"],
            json!("IDLE"),
            "case {case:?}: expected an unused group to stay IDLE, got {:?}",
            group["state"]
        );

        // Round trip: the update persisted, so the reader reports exactly what
        // the update reported.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInputSecurityGroup: expected 200, got {status}: {described}"
        );
        for member in [
            "arn",
            "id",
            "inputs",
            "state",
            "tags",
            "whitelistRules",
            "channels",
        ] {
            assert_eq!(
                described[member], group[member],
                "case {case:?}: expected describe to report the updated {member} {:?}, got {:?}",
                group[member], described[member]
            );
        }
    }
}

/// Rule: an update is applied to the group the path names and to no other, and
/// the last update of a group is the one that stands.
#[tokio::test]
async fn an_update_touches_only_the_group_it_names() {
    let router = medialive();
    let target = create_group(
        &router,
        "the updated group",
        json!({ "whitelistRules": [{ "cidr": "10.1.0.0/16" }] }),
    )
    .await;
    let target_id = target["id"].as_str().unwrap_or_default().to_string();
    let bystander = create_group(
        &router,
        "the untouched group",
        json!({ "whitelistRules": [{ "cidr": "10.2.0.0/16" }], "tags": { "keep": "me" } }),
    )
    .await;
    let bystander_id = bystander["id"].as_str().unwrap_or_default().to_string();

    // Successive updates of one group: each one replaces what the one before it
    // wrote, so the last stands.
    for (case, cidr) in [("first update", "10.3.0.0/16"), ("second update", "10.4.0.0/16")] {
        let (status, updated) = update_group(
            &router,
            &target_id,
            json!({ "whitelistRules": [{ "cidr": cidr }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["securityGroup"]["whitelistRules"],
            whitelist(&[cidr]),
            "case {case:?}: expected {cidr:?}, got {:?}",
            updated["securityGroup"]["whitelistRules"]
        );
    }

    let (status, described) = describe_group(&router, &target_id).await;
    assert_eq!(status, 200, "expected the updated group to be readable, got {status}: {described}");
    assert_eq!(
        described["whitelistRules"],
        whitelist(&["10.4.0.0/16"]),
        "case \"the last update stands\": expected 10.4.0.0/16, got {:?}",
        described["whitelistRules"]
    );

    let (status, other) = describe_group(&router, &bystander_id).await;
    assert_eq!(status, 200, "expected the other group to be readable, got {status}: {other}");
    assert_eq!(
        other["whitelistRules"],
        whitelist(&["10.2.0.0/16"]),
        "case \"the group the update did not name\": expected its own rule, got {:?}",
        other["whitelistRules"]
    );
    assert_eq!(
        other["tags"],
        json!({ "keep": "me" }),
        "case \"the group the update did not name\": expected its own tags, got {:?}",
        other["tags"]
    );
}

/// Rule: a whitelist rule must name an IPv4 CIDR with a prefix length of 0 to 32
/// on an update exactly as on a create, and a request carrying anything else is
/// refused with the operation's modelled `BadRequestException` (400) while the
/// group keeps the whitelist it already had.
///
/// The same table of out-of-rule values the create suite uses, because it is the
/// same rule: the member is documented "List of IPv4 CIDR addresses to whitelist"
/// on both operations. The last row proves the whole list is checked before
/// anything is written — its first entry is valid and its second is not.
#[tokio::test]
async fn an_update_refuses_rules_outside_the_ipv4_cidr_rule() {
    // (case, the whitelistRules the update carries)
    let table: [(&str, Value); 10] = [
        ("bare address, no prefix", json!([{ "cidr": "10.0.0.0" }])),
        ("prefix one past the bound", json!([{ "cidr": "10.0.0.0/33" }])),
        ("negative prefix", json!([{ "cidr": "10.0.0.0/-1" }])),
        ("non-numeric prefix", json!([{ "cidr": "10.0.0.0/x" }])),
        ("octet out of range", json!([{ "cidr": "256.0.0.0/8" }])),
        ("too few octets", json!([{ "cidr": "10.0.0/8" }])),
        ("an IPv6 prefix", json!([{ "cidr": "2001:db8::/32" }])),
        ("empty string", json!([{ "cidr": "" }])),
        ("a rule that names no CIDR", json!([{}])),
        (
            "a valid rule followed by an invalid one",
            json!([{ "cidr": "10.9.0.0/16" }, { "cidr": "nonsense" }]),
        ),
    ];

    for (case, rules) in table {
        let router = medialive();
        let created = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
        )
        .await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, body) = update_group(&router, &id, json!({ "whitelistRules": rules })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected the operation's own BadRequestException, got {body}"
        );

        // A refused update writes nothing: the group still has the rule the
        // create gave it, not a prefix of the refused list.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            whitelist(&["10.0.0.0/16"]),
            "case {case:?}: expected the refused update to leave the whitelist alone, got {:?}",
            described["whitelistRules"]
        );
    }
}

/// Rule: `UpdateInputSecurityGroup` on an id no group has answers the operation's
/// modelled `NotFoundException` (404) and writes nothing.
///
/// The table spans the kinds of id that name no group, including a channel's and
/// an input's — the id spaces are separate, so neither is a group's id.
#[tokio::test]
async fn an_update_refuses_an_id_no_group_has() {
    let router = medialive();
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "not-a-security-group",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({ "name": "also-not-a-security-group", "type": "URL_PULL",
                      "sources": pull_sources() })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let input_id = created_input["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // One group exists, so the store is not simply empty.
    let real = create_group(&router, "a real group", json!({})).await;
    let real_id = real["id"].as_str().unwrap_or_default().to_string();

    // (case, the id to update)
    let table: [(&str, String); 5] = [
        ("zero", "0".to_string()),
        ("a numeric id never minted", "9999999".to_string()),
        ("a non-numeric id", "not-an-id".to_string()),
        ("a channel's id", channel_id),
        ("an input's id", input_id),
    ];

    for (case, id) in table {
        let (status, body) = update_group(
            &router,
            &id,
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/8" }] }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The group that does exist was not touched by any of those refusals.
    let (status, described) = describe_group(&router, &real_id).await;
    assert_eq!(status, 200, "expected the real group to be readable, got {status}: {described}");
    assert_eq!(
        described["whitelistRules"],
        json!([]),
        "case \"the group no refused update named\": expected no rules, got {:?}",
        described["whitelistRules"]
    );
}

/// Rule: a group's `State`, `Inputs` and `Channels` report what currently uses
/// it, so an update to a group a resource references succeeds and keeps reporting
/// that use — the update changes the whitelist, not who uses the group.
#[tokio::test]
async fn an_update_keeps_reporting_what_uses_the_group() {
    let router = medialive();
    let group = create_group(
        &router,
        "in use",
        json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
    )
    .await;
    let group_id = group["id"].as_str().unwrap_or_default().to_string();

    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": "uses-the-group",
            "type": "URL_PULL",
            "sources": pull_sources(),
            "inputSecurityGroups": [group_id.clone()],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let input_id = created_input["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, updated) = update_group(
        &router,
        &group_id,
        json!({ "whitelistRules": [{ "cidr": "198.51.100.0/24" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an input uses the group\": expected the update to succeed, got {status}: {updated}"
    );
    let group = updated["securityGroup"].clone();
    assert_eq!(
        group["whitelistRules"],
        whitelist(&["198.51.100.0/24"]),
        "case \"an input uses the group\": expected the new rule, got {:?}",
        group["whitelistRules"]
    );
    assert_eq!(
        group["state"],
        json!("IN_USE"),
        "case \"an input uses the group\": expected IN_USE, got {:?}",
        group["state"]
    );
    assert_eq!(
        group["inputs"],
        json!([input_id]),
        "case \"an input uses the group\": expected the using input's id, got {:?}",
        group["inputs"]
    );
    assert_eq!(
        group["channels"],
        json!([]),
        "case \"an input uses the group\": expected no channels, got {:?}",
        group["channels"]
    );

    // The clock's own transitions still settle around an update: the channel the
    // group is attached to goes on moving through its states, and once it is
    // deleted the group reports that it is no longer used.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "also-uses-the-group",
            "channelSecurityGroups": [group_id.clone()],
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, updated) = update_group(&router, &group_id, json!({ "tags": { "phase": "attached" } })).await;
    assert_eq!(
        updated["securityGroup"]["channels"],
        json!([channel_id.clone()]),
        "case \"a channel attached too\": expected the attaching channel's id, got {:?}",
        updated["securityGroup"]["channels"]
    );
    assert_eq!(
        updated["securityGroup"]["whitelistRules"],
        whitelist(&["198.51.100.0/24"]),
        "case \"a channel attached too\": expected a tags-only update to keep the whitelist, got {:?}",
        updated["securityGroup"]["whitelistRules"]
    );

    wait_for_the_clock();
    let (status, deleting) = call(
        &router,
        "DELETE",
        &format!("/prod/channels/{channel_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteChannel: expected 200, got {status}: {deleting}");
    wait_for_the_clock();

    let (_, updated) = update_group(&router, &group_id, json!({ "tags": { "phase": "released" } })).await;
    assert_eq!(
        updated["securityGroup"]["channels"],
        json!([]),
        "case \"the channel deleted\": expected no channels, got {:?}",
        updated["securityGroup"]["channels"]
    );
    assert_eq!(
        updated["securityGroup"]["tags"],
        json!({ "phase": "released" }),
        "case \"the channel deleted\": expected the update's tags, got {:?}",
        updated["securityGroup"]["tags"]
    );
}

/// Rule: a MediaLive operation that mints a new resource answers HTTP 201
/// Created; every other operation answers the status its model declares.
///
/// The expected status of each row is the one the model declares for that
/// operation (`smithy_query(query="operation", operation="…")`) — except
/// `CreateInputSecurityGroup`, where the model says 200 and the recorded wire
/// response says 201, and the wire is the authority. The rows span the service's
/// implemented resource creates, because the rule is about minting a resource
/// rather than about any one of them.
#[tokio::test]
async fn a_resource_create_answers_the_status_the_wire_carries() {
    // (case, URI, request body, expected status, where that status comes from)
    let table: [(&str, &str, Value, u16, &str); 4] = [
        (
            "input security group",
            "/prod/inputSecurityGroups",
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/8" }] }),
            201,
            "the recorded wire response (the model's 200 is stale)",
        ),
        (
            "channel",
            "/prod/channels",
            json!({
                "name": "created",
                "encoderSettings": crate::support::channel_encoder_settings(),
            }),
            201,
            "the model's declared code",
        ),
        (
            "input",
            "/prod/inputs",
            json!({ "name": "created", "type": "URL_PULL", "sources": pull_sources() }),
            201,
            "the model's declared code",
        ),
        (
            // The model declares 200 for this one too, and mainline's own fix
            // put it at 201 on the same recorded-wire evidence.
            "sdi source",
            "/prod/sdiSources",
            json!({ "name": "created", "type": "SINGLE" }),
            201,
            "the recorded wire response (the model's 200 is stale)",
        ),
    ];

    for (case, uri, body, expected, source) in table {
        let router = medialive();
        let (status, response) = call(&router, "POST", uri, Some(body)).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} from {source}, got {status}: {response}"
        );
    }
}

/// Rule: a MediaLive delete answers with the resource's own representation, read
/// out of the store and never assembled from the request, and the state the
/// delete writes is what every read AFTER the call reports. A group has no
/// transient deletion state of its own (`InputSecurityGroupState`: IDLE | IN_USE
/// | UPDATING | DELETED), so `DeleteInputSecurityGroup` answers with the state
/// the group's USERS imply — `IDLE` while nothing references it, `IN_USE` while
/// something does, for a group this call deletes and for one an earlier call
/// already deleted — while `DescribeInputSecurityGroup` reports the terminal
/// `DELETED` from the delete onwards: a client polling for the terminal state
/// sees it, and one that treats `IDLE` as "available" cannot attach a deleted
/// group. The delete is idempotent and touches no other group.
///
/// The rows differ along what the rule generalises over: how many whitelist
/// rules the group carries, whether it has tags, and WHAT CURRENTLY USES IT —
/// nothing (so the group was `IDLE`), a channel (so it was `IN_USE`), or an
/// input whose own delete was accepted first (so it is `IDLE` again) — because
/// the terminal state takes precedence over the state derived from the group's
/// users on a read, and the derived one is what the delete reports. No row
/// deletes a group an input still uses: that delete is refused with its own
/// conflict (the `input_attachment_lifecycle` suite covers that refusal).
#[tokio::test]
async fn a_deleted_group_reaches_the_terminal_deleted_state() {
    // Every member `DescribeInputSecurityGroupResponse` declares
    // (`smithy_query(query="operation", operation="DescribeInputSecurityGroup")`),
    // as the wire names them.
    const ALWAYS_REPORTED: [&str; 7] = [
        "arn",
        "id",
        "inputs",
        "state",
        "tags",
        "whitelistRules",
        "channels",
    ];

    // (case, the group's create body, what references it before the delete,
    //  the state it reports before the delete)
    let table: [(&str, Value, Option<&str>, &str); 5] = [
        ("no rules, nothing uses it", json!({}), None, "IDLE"),
        (
            "one rule, nothing uses it",
            json!({ "whitelistRules": [{ "cidr": "192.0.2.1/32" }] }),
            None,
            "IDLE",
        ),
        (
            "several rules and tags, nothing uses it",
            json!({
                "whitelistRules": [{ "cidr": "0.0.0.0/0" }, { "cidr": "10.0.0.0/8" }],
                "tags": { "env": "test" },
            }),
            None,
            "IDLE",
        ),
        (
            "an input used it and was deleted",
            json!({ "whitelistRules": [{ "cidr": "198.51.100.0/24" }] }),
            Some("deleted input"),
            "IDLE",
        ),
        (
            "a channel uses it",
            json!({ "whitelistRules": [{ "cidr": "203.0.113.0/24" }] }),
            Some("channel"),
            "IN_USE",
        ),
    ];

    for (case, create_with, referenced_by, state_before) in table {
        let router = medialive();
        // A second group nothing deletes, so the suite can tell one group's
        // deletion from a store-wide change.
        let bystander = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": "10.255.0.0/16" }] }),
        )
        .await;
        let bystander_id = bystander["id"].as_str().unwrap_or_default().to_string();

        let created = create_group(&router, case, create_with).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        match referenced_by {
            Some("deleted input") => {
                let (status, body) = call(
                    &router,
                    "POST",
                    "/prod/inputs",
                    Some(json!({
                        "name": "used-the-group",
                        "type": "URL_PULL",
                        "sources": pull_sources(),
                        "inputSecurityGroups": [id.clone()],
                    })),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: CreateInput: expected 201, got {status}: {body}"
                );
                let input_id = body["input"]["id"].as_str().unwrap_or_default().to_string();
                // Accepting the input's delete releases the group, so the group
                // has no current user by the time the group's delete runs.
                let (status, body) =
                    call(&router, "DELETE", &format!("/prod/inputs/{input_id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteInput: expected 200, got {status}: {body}"
                );
                wait_for_the_clock();
            }
            Some("channel") => {
                let (status, body) = call(
                    &router,
                    "POST",
                    "/prod/channels",
                    Some(json!({
                        "name": "uses-the-group",
                        "channelSecurityGroups": [id.clone()],
                        "encoderSettings": crate::support::channel_encoder_settings(),
                    })),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: CreateChannel: expected 201, got {status}: {body}"
                );
            }
            _ => {}
        }

        assert_eq!(
            state_of(&router, case, &id).await,
            state_before,
            "case {case:?}: expected the group to report {state_before:?} before the delete"
        );
        let (_, before) = describe_group(&router, &id).await;

        let (status, deleted) = delete_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteInputSecurityGroup: expected 200, got {status}: {deleted}"
        );

        // The body is the group, not an empty object, and its state is the one its
        // users imply — the delete's own effect is what every later read reports.
        assert!(
            deleted.as_object().is_some_and(|members| !members.is_empty()),
            "case {case:?}: expected the group's representation, got {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!(state_before),
            "case {case:?}: expected the delete to report the state its users imply, got {:?}",
            deleted["state"]
        );
        for member in ALWAYS_REPORTED {
            assert!(
                !deleted[member].is_null(),
                "case {case:?}: expected the delete to report {member}, got {deleted}"
            );
            assert_eq!(
                deleted[member], before[member],
                "case {case:?}: expected the delete's {member} to be the group's own {:?}, got {:?}",
                before[member], deleted[member]
            );
        }

        // The terminal state is reported straight away: a deleted group owes the
        // clock nothing.
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETED",
            "case {case:?}: expected the deleted group to report the terminal DELETED"
        );

        // The record stays in the account: everything it stored still reads back.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deleted group to stay readable, got {status}: {described}"
        );
        for member in ["arn", "id", "whitelistRules", "tags"] {
            assert_eq!(
                described[member], created[member],
                "case {case:?}: expected the deleted group to keep its {member} {:?}, got {:?}",
                created[member], described[member]
            );
        }

        // Idempotent: a repeated delete neither fails nor revives the group, and
        // answers with the same group and the same user-derived state.
        let (status, again) = delete_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated delete to succeed, got {status}: {again}"
        );
        assert_eq!(
            again["state"],
            json!(state_before),
            "case {case:?}: expected a repeated delete to report the users' state too, got {:?}",
            again["state"]
        );
        assert_eq!(
            again["id"], deleted["id"],
            "case {case:?}: expected a repeated delete to report the same group, got {:?}",
            again["id"]
        );
        assert_eq!(
            state_of(&router, case, &id).await,
            "DELETED",
            "case {case:?}: expected a repeated delete to leave the group DELETED"
        );

        // The group nothing deleted is untouched.
        assert_eq!(
            state_of(&router, case, &bystander_id).await,
            "IDLE",
            "case {case:?}: expected the group nothing deleted to stay IDLE"
        );
        let (_, other) = describe_group(&router, &bystander_id).await;
        assert_eq!(
            other["whitelistRules"],
            whitelist(&["10.255.0.0/16"]),
            "case {case:?}: expected the untouched group to keep its own rule, got {:?}",
            other["whitelistRules"]
        );
    }
}

/// Rule: `DeleteInputSecurityGroup` on an id no group has answers the
/// operation's modelled `NotFoundException` (404) and deletes nothing.
///
/// The table spans the kinds of id that name no group, including a channel's and
/// an input's — the id spaces are separate, so neither is a group's id.
#[tokio::test]
async fn deleting_an_id_no_group_has_is_not_found() {
    let router = medialive();
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "not-a-security-group",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({ "name": "also-not-a-security-group", "type": "URL_PULL",
                      "sources": pull_sources() })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let input_id = created_input["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // One group exists, so the refusals are about the id and not about an empty
    // store.
    let real = create_group(&router, "a real group", json!({})).await;
    let real_id = real["id"].as_str().unwrap_or_default().to_string();

    // (case, the id to delete)
    let table: [(&str, String); 5] = [
        ("zero", "0".to_string()),
        ("a numeric id never minted", "9999999".to_string()),
        ("a non-numeric id", "not-an-id".to_string()),
        ("a channel's id", channel_id),
        ("an input's id", input_id),
    ];

    for (case, id) in table {
        let (status, body) = delete_group(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The group that does exist was deleted by none of those refusals.
    assert_eq!(
        state_of(&router, "the group no refused delete named", &real_id).await,
        "IDLE",
        "expected the real group to be untouched by the refused deletes"
    );
}
