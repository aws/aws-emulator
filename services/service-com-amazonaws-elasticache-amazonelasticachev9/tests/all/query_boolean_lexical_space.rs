// SPDX-License-Identifier: CC0-1.0

//! The lexical space a BOOLEAN member of this front door's requests is carried in, and the
//! sentence text outside it is refused with.
//!
//! The rule under test: on this wire a boolean member is the TEXT of a form field, and the
//! text that spells a boolean is the lexical space of an XML Schema boolean — `true`, `false`,
//! `1` and `0`, read after the whitespace around the value is stripped — so a request spelling
//! a boolean either way is served as the boolean it spells, and text in no spelling of the
//! space is refused 400 while the request is READ, with the protocol layer's own fixed
//! sentence: no member name, no value from the request, and no accepted-literal list narrower
//! than the space. It is a fact of the protocol rather than of any operation, so the same
//! reading and the same refusal hold for every boolean member of every operation, at any depth
//! of the request.
//!
//! The tests are tables. The rows differ along what the rule generalises over — which
//! operation and which member carries the boolean, whether that member sits at the top of the
//! request or inside an element of a list of structures, which spelling of the space the value
//! is in (including one padded with whitespace), and how text outside the space misses it (a
//! word, a word in the wrong case, a number outside the space, a decimal, whitespace alone).
//!
//! Every assertion goes over the wire through the same router the production emulator mounts,
//! using the harness in [`crate::harness`]; nothing reaches into the service's state struct.

use crate::harness::{params, poll_until, Emulator, REGION};

/// The sentence the protocol layer refuses a boolean member's text that spells no boolean
/// with. Entirely fixed text: whichever member of whichever operation carried the value, and
/// whatever that value was.
const NON_BOOLEAN: &str = "boolean must follow xsd1.1 definition";

/// One row: an operation, the form pairs it needs beside the boolean under test, and the key
/// the boolean is carried under.
struct BooleanMember {
    case: &'static str,
    action: &'static str,
    rest: &'static [(&'static str, &'static str)],
    member: &'static str,
}

/// The axis: which operation reads the boolean, and WHERE the member sits in the request —
/// the last row carries it inside an element of a list of structures, which only the walk
/// itself can find.
const BOOLEAN_MEMBERS: &[BooleanMember] = &[
    BooleanMember {
        case: "the node-info flag of a cluster lookup",
        action: "DescribeCacheClusters",
        rest: &[],
        member: "ShowCacheNodeInfo",
    },
    BooleanMember {
        case: "the standalone-only flag of a cluster lookup",
        action: "DescribeCacheClusters",
        rest: &[],
        member: "ShowCacheClustersNotInReplicationGroups",
    },
    BooleanMember {
        case: "the minor-version flag of a cluster create",
        action: "CreateCacheCluster",
        rest: &[
            ("CacheClusterId", "boolean-row-cluster"),
            // A create that provisions its own nodes names the node type they
            // run on; this row is about the boolean beside it.
            ("CacheNodeType", "cache.t3.micro"),
        ],
        member: "AutoMinorVersionUpgrade",
    },
    BooleanMember {
        case: "the failover flag of a replication group create",
        action: "CreateReplicationGroup",
        rest: &[
            ("ReplicationGroupId", "boolean-row-group"),
            ("ReplicationGroupDescription", "a boolean row"),
        ],
        member: "AutomaticFailoverEnabled",
    },
    BooleanMember {
        case: "the apply-immediately flag of a settings change",
        action: "ModifyReplicationGroup",
        rest: &[("ReplicationGroupId", "boolean-row-group")],
        member: "ApplyImmediately",
    },
    BooleanMember {
        case: "a flag inside an element of a list of structures",
        action: "CreateReplicationGroup",
        rest: &[
            ("ReplicationGroupId", "boolean-row-nested"),
            ("ReplicationGroupDescription", "a nested boolean row"),
            (
                "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.LogType",
                "slow-log",
            ),
        ],
        member: "LogDeliveryConfigurations.LogDeliveryConfigurationRequest.1.Enabled",
    },
];

/// Text in no spelling of the space, and HOW each misses it.
const OUTSIDE_THE_SPACE: &[(&str, &str)] = &[
    ("a word of assent", "yes"),
    ("a word of refusal", "no"),
    ("the word in upper case", "TRUE"),
    ("the word in title case", "False"),
    ("a number outside the space", "2"),
    ("a negative number", "-1"),
    ("a decimal", "1.0"),
    ("whitespace alone", " "),
];

#[tokio::test]
async fn a_boolean_member_outside_the_lexical_space_is_refused_with_the_protocols_sentence() {
    for row in BOOLEAN_MEMBERS {
        let emulator = Emulator::new();
        let case = row.case;
        for (how, value) in OUTSIDE_THE_SPACE {
            let mut request = params(&[(row.member, *value)]);
            request.extend(params(row.rest));
            let refused = emulator.call(row.action, &request).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} / {how}: expected HTTP 400, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_message(),
                NON_BOOLEAN,
                "case {case:?} / {how}: expected {NON_BOOLEAN:?}, got {:?}",
                refused.error_message()
            );
            // The sentence is the protocol's: it names neither the member whose text could
            // not be read nor the text itself.
            assert!(
                !refused.error_message().contains(row.member),
                "case {case:?} / {how}: the sentence must not name the member, got {:?}",
                refused.error_message()
            );
            // Whitespace is what the value's own facet strips, so a value made of it alone
            // is the one row whose text is a substring of any sentence; every other row
            // proves the sentence echoes nothing of the request.
            if !value.trim().is_empty() {
                assert!(
                    !refused.error_message().contains(value),
                    "case {case:?} / {how}: the sentence must not echo the value, got {:?}",
                    refused.error_message()
                );
            }
        }
    }
}

#[tokio::test]
async fn every_spelling_of_the_space_is_read_as_the_boolean_it_spells() {
    let emulator = Emulator::new();
    // One cluster to report on, so the flag under test decides what the answer CARRIES rather
    // than whether there is anything to answer with.
    let created = emulator
        .call(
            "CreateCacheCluster",
            &crate::harness::cluster_create_params("spelled-flags", &[]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the cluster under test must be created, got {} {}",
        created.status, created.body
    );

    // The node list is part of a cluster once the clock has settled it, so the suite looks
    // again until the cluster reports `available` rather than waiting a fixed time.
    let settled = poll_until(
        &emulator,
        REGION,
        "the cluster under test",
        "the cluster settling at available",
        "DescribeCacheClusters",
        &params(&[("CacheClusterId", "spelled-flags")]),
        |reply| reply.text("CacheClusterStatus") == Some("available"),
    )
    .await;
    assert_eq!(
        settled.text("CacheClusterStatus"),
        Some("available"),
        "the cluster must settle available, got {}",
        settled.body
    );

    // `ShowCacheNodeInfo` is "an optional flag that can be included … to retrieve information
    // about the individual cache nodes", so the node list is in the answer exactly when the
    // flag read TRUE — which is what makes each spelling observable over the wire.
    for (how, value, expected_nodes) in [
        ("the word true", "true", true),
        ("the word false", "false", false),
        ("the numeral one", "1", true),
        ("the numeral zero", "0", false),
        ("a word padded with whitespace", " true ", true),
        ("a numeral padded with whitespace", " 0 ", false),
    ] {
        let described = emulator
            .call(
                "DescribeCacheClusters",
                &params(&[
                    ("CacheClusterId", "spelled-flags"),
                    ("ShowCacheNodeInfo", value),
                ]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {how:?}: a spelling of the space must be served, got {} {}",
            described.status, described.body
        );
        let reported = !described.all("CacheNodes/CacheNode/CacheNodeId").is_empty();
        assert_eq!(
            reported, expected_nodes,
            "case {how:?}: expected the node list reported={expected_nodes}, got {reported} in {}",
            described.body
        );
    }
}
