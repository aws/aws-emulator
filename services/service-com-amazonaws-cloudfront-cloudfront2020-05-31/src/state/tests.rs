// SPDX-License-Identifier: CC0-1.0

//! The unit tests of the records, predicates and message producers this service's state
//! holds. The end-to-end behaviour of every operation over that state is pinned by the
//! crate's own `tests/all/` suites; what is tested here is the state module's own
//! vocabulary.

use super::*;
use crate::ops::types::{Aliases, Origin, Origins};

/// The one producer of the identifier-shape sentence and the `&'static str` form the
/// test suites assert cannot drift apart: both spell the same text for `Id`, and the
/// producer spells it for any other member name the same way.
#[test]
fn id_format_invalid_message_matches_the_constant_form() {
    assert_eq!(
        id_format_invalid_message("Id"),
        RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE,
        "case Id: expected the constant form {RESPONSE_HEADERS_POLICY_ID_FORMAT_MESSAGE:?}, \
             got {:?}",
        id_format_invalid_message("Id")
    );
    assert_eq!(
        id_format_invalid_message("DistributionTenantId"),
        "DistributionTenantId format is invalid",
        "case DistributionTenantId: expected the member's own sentence, got {:?}",
        id_format_invalid_message("DistributionTenantId")
    );
}

/// Each MINT's predicate admits exactly what this service mints for that kind — the
/// resume points a listing issues are its own ids — while the form an operation
/// ADDRESSES a resource with ([`is_namespaced_resource_id`]) admits every namespace an
/// opaque id is written under, whatever label, width and alphabet it takes, and nothing
/// that carries no namespace at all.
#[test]
fn an_id_predicate_admits_only_the_shape_its_mint_renders() {
    let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
    for _ in 0..100 {
        let list = state.allocate_anycast_ip_list_id();
        assert!(
            is_namespaced_resource_id(&list),
            "case allocate_anycast_ip_list_id: expected {list:?} to be an addressable id"
        );
        assert!(
            !is_namespaced_resource_id(&list.to_uppercase()),
            "case upper-cased: expected {list:?} upper-cased not to be an addressable id, \
                 since a namespace label is lowercase"
        );
        assert!(
            is_namespaced_resource_id(&list[..list.len() - 1]),
            "case one digit short: expected {list:?} truncated to still be an addressable id, \
                 since the width is one observed id's rather than the rule"
        );

        let tenant = state.allocate_distribution_tenant_id();
        assert!(
            is_distribution_tenant_id(&tenant),
            "case allocate_distribution_tenant_id: expected {tenant:?} to be a tenant id"
        );
        assert!(
            !is_distribution_tenant_id(&format!("{tenant}-")),
            "case outside the alphabet: expected {tenant:?} with a separator appended not to \
                 be a tenant id, since an opaque body carries none"
        );
        assert!(
            is_namespaced_resource_id(&tenant)
                && is_namespaced_resource_id(&tenant.to_lowercase()),
            "case addressing {tenant:?}: expected both cases of the body to be addressable, \
                 since the body's alphabet is this mint's rather than the rule"
        );

        let group = state.allocate_key_group_id();
        assert!(
            is_key_group_id(&group),
            "case allocate_key_group_id: expected {group:?} to be a key group id"
        );
        assert!(
            !is_key_group_id(&group.to_uppercase()),
            "case upper-cased: expected {group:?} upper-cased not to be a key group id"
        );

        let policy = state.allocate_continuous_deployment_policy_id();
        assert!(
            is_continuous_deployment_policy_id(&policy),
            "case allocate_continuous_deployment_policy_id: expected {policy:?} to be a \
                 policy id"
        );
    }

    // Across the kinds: no id of one kind is read as an id of another, and no bare word
    // is read as an id at all.
    for value in ["EDFDVBD6EXAMPLE", "nope", "-", ""] {
        assert!(
            !is_namespaced_resource_id(value),
            "case {value:?}: expected it not to be an addressable namespaced id"
        );
        assert!(
            !is_distribution_tenant_id(value),
            "case {value:?}: expected it not to be a distribution tenant id"
        );
        assert!(
            !is_key_group_id(value),
            "case {value:?}: expected it not to be a key group id"
        );
    }

    // The addressable form spans the namespaces CloudFront writes an opaque id under:
    // another label, another body width, another body alphabet and another separator are
    // each an id the store decides on.
    for value in [
        "dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa",
        "aip_1A2B3C4D5E6F7G8H9J0K1L",
        "ipl-notaminted-id",
        "cg_ZZZZZZZZZZZZZ",
        "zz_0",
    ] {
        assert!(
            is_namespaced_resource_id(value),
            "case {value:?}: expected a namespaced id to be addressable"
        );
    }

    // And it refuses what carries no namespace: no separator at all, an empty or
    // over-long label, an upper-case label, a body that is empty or outside the
    // identifier alphabet.
    let grossly_malformed = "A".repeat(512);
    for value in [
        grossly_malformed.as_str(),
        "iplist0123456789",
        "-ipl-0",
        "present-list",
        "IPL-0123456789abcdef0",
        "dt_",
        "dt_token with a space",
        "f9a6a0b4-8d39-4a1e-9f67-1b0f7f5d2c3e",
    ] {
        assert!(
            !is_namespaced_resource_id(value),
            "case {value:?}: expected a value of no namespace not to be addressable"
        );
    }
}

/// The mint never repeats a token: distinct counter values render distinctly, which
/// is what makes two creates two distributions and every mutation a fresh version.
#[test]
fn minted_tokens_are_distinct() {
    let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..1_000 {
        let id = state.allocate_distribution_id();
        assert!(
            is_distribution_id(&id),
            "case allocate_distribution_id: expected an `E`+13 id, got {id:?}"
        );
        assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated a token");
        let etag = state.next_etag();
        assert!(
            is_version_token(&etag),
            "case next_etag: expected an `E`+13 version token, got {etag:?}"
        );
        assert!(seen.insert(etag.clone()), "case {etag:?}: the mint repeated a token");
    }
}

/// A minted key value store id is a UUID and never repeats: the id is the resource
/// part of the store's ARN, which is how the data plane addresses one store.
#[test]
fn minted_key_value_store_ids_are_distinct_uuids() {
    let mut state = ComAmazonawsCloudfrontCloudfront20200531State::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..1_000 {
        let id = state.allocate_key_value_store_id();
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case allocate_key_value_store_id: expected 8-4-4-4-12 groups, got {id:?}"
        );
        assert!(
            id.bytes()
                .all(|b| b == b'-' || (b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "case allocate_key_value_store_id: expected lowercase hex, got {id:?}"
        );
        assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated an id");
    }
}

/// A minted request id has the shape an AWS request id takes and names ONE response:
/// the envelope's whole requirement is that a failure can be correlated, so two
/// responses must never carry the same id.
#[test]
fn minted_request_ids_are_distinct_and_shaped() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..10_000 {
        let id = new_request_id();
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case new_request_id: expected 8-4-4-4-12 groups, got {id:?}"
        );
        assert!(
            id.bytes()
                .all(|b| b == b'-' || (b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "case new_request_id: expected lowercase hex, got {id:?}"
        );
        assert!(seen.insert(id.clone()), "case {id:?}: the mint repeated a request id");
    }
}

/// An `If-Match` is current, stale or not a version at all — and nothing else.
#[test]
fn if_match_classifies_every_header_form() {
    let current = "E2QWRUHAPOMQZL";
    let cases: &[(Option<&str>, IfMatch)] = &[
        (None, IfMatch::Invalid),
        (Some(""), IfMatch::Invalid),
        (Some("not-a-version"), IfMatch::Invalid),
        (Some("E2QWRUHAPOMQZ"), IfMatch::Invalid),
        (Some(current), IfMatch::Current),
        (Some("\"E2QWRUHAPOMQZL\""), IfMatch::Current),
        (Some("E3QWRUHAPOMQZL"), IfMatch::Stale),
    ];
    for (header, expected) in cases {
        let actual = check_if_match(current, *header);
        assert_eq!(
            actual, *expected,
            "case If-Match={header:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A configuration a client could submit, as the baseline the cases below differ
/// from one member at a time.
fn a_config() -> DistributionConfig {
    DistributionConfig {
        caller_reference: "ref-baseline".to_string(),
        comment: "the baseline".to_string(),
        enabled: true,
        origins: Origins {
            quantity: 1,
            items: vec![Origin {
                id: "primary".to_string(),
                domain_name: "origin.example.com".to_string(),
                ..Origin::default()
            }],
        },
        default_cache_behavior: DefaultCacheBehavior {
            target_origin_id: "primary".to_string(),
            viewer_protocol_policy: "allow-all".to_string(),
            ..DefaultCacheBehavior::default()
        },
        ..DistributionConfig::default()
    }
}

/// [`same_config`] separates exactly the configurations that differ, over every kind
/// of member a `DistributionConfig` carries: a required scalar, the `@sensitive`
/// `Comment`, a boolean, an optional scalar the baseline leaves out, a nested
/// structure, and a list wrapper's `Quantity` and `Items`.
///
/// This is what the create path's idempotency rests on: a comparison that could not
/// see one of these members would answer a second, DIFFERENT configuration with the
/// first distribution instead of the modelled conflict.
#[test]
fn same_config_separates_configurations_that_differ() {
    let baseline = a_config();
    assert!(
        same_config(&baseline, &a_config()),
        "case identical: expected two equal configurations to compare the same"
    );

    let mut differing: Vec<(&str, DistributionConfig)> = Vec::new();
    differing.push((
        "caller-reference",
        DistributionConfig {
            caller_reference: "ref-other".to_string(),
            ..a_config()
        },
    ));
    differing.push((
        "sensitive-comment",
        DistributionConfig {
            comment: "a different comment".to_string(),
            ..a_config()
        },
    ));
    differing.push((
        "empty-comment",
        DistributionConfig {
            comment: String::new(),
            ..a_config()
        },
    ));
    differing.push((
        "enabled",
        DistributionConfig {
            enabled: false,
            ..a_config()
        },
    ));
    differing.push((
        "optional-scalar-named",
        DistributionConfig {
            price_class: Some(DEFAULT_PRICE_CLASS.to_string()),
            ..a_config()
        },
    ));
    differing.push((
        "nested-structure",
        DistributionConfig {
            default_cache_behavior: DefaultCacheBehavior {
                viewer_protocol_policy: "https-only".to_string(),
                ..a_config().default_cache_behavior
            },
            ..a_config()
        },
    ));
    differing.push((
        "list-wrapper-items",
        DistributionConfig {
            origins: Origins {
                quantity: 1,
                items: vec![Origin {
                    id: "primary".to_string(),
                    domain_name: "other.example.com".to_string(),
                    ..Origin::default()
                }],
            },
            ..a_config()
        },
    ));
    differing.push((
        "list-wrapper-quantity",
        DistributionConfig {
            origins: Origins {
                quantity: 2,
                ..a_config().origins
            },
            ..a_config()
        },
    ));
    differing.push((
        "absent-wrapper-becomes-empty",
        DistributionConfig {
            aliases: Some(Aliases {
                quantity: 0,
                items: None,
            }),
            ..a_config()
        },
    ));

    for (case, other) in &differing {
        assert!(
            !same_config(&baseline, other),
            "case {case}: expected a configuration differing in {case} to compare \
                 DIFFERENT, got the same; baseline {baseline:?} vs {other:?}"
        );
        assert!(
            same_config(other, other),
            "case {case}: expected a configuration to compare the same as itself"
        );
    }
}
