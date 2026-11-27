// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the integrations a serverless cache carries:
//! `CreateServerlessCache$Integrations`, what `ServerlessCache$Integrations`
//! reports for them, and the `Engine` / `MajorEngineVersion` pair one needs.
//!
//! The rule under test: a cache reports the integrations it was created with from
//! persisted state — one per `IntegrationType`, each with the status that
//! integration has reached (`enabling` while the service brings it up, `enabled`
//! once the clock has carried that through) — a cache created with none reports no
//! integration at all, and an integration is only configured on a cache whose
//! engine and major engine version can carry it ("DynamoDB integration requires
//! the Valkey engine with a major engine version of 9 or above",
//! `CreateServerlessCache$Integrations`), whether it is the create that asks for
//! one or the modify that would move a cache carrying one onto another pair.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass.
//!
//! The tests are tables: the rows span how many integrations the request names,
//! the position they arrive at inside the list, whether the same type is named
//! twice, whether any is named at all, which call the answer is read off, and —
//! for the refusals — the engine and the version line the request would leave the
//! cache on.

use crate::harness::{
    carries_element, hold_pending_changes, params, poll_until, release_pending_changes, Emulator,
    Reply, REGION,
};

/// The `IntegrationType` this API offers, as the modelled `IntegrationType` enum
/// declares it (`smithy_query(query="shape", shape="IntegrationType")`:
/// `dynamodb`). Written out rather than read from the service's own constant: a
/// test that built its expectation from the constant would pass whatever that
/// constant said.
const DYNAMODB: &str = "dynamodb";

/// The engine and major version line a cache has to run to carry an integration,
/// as `CreateServerlessCache$Integrations` documents it: "DynamoDB integration
/// requires the Valkey engine with a major engine version of 9 or above."
const INTEGRATION_ENGINE: &[(&str, &str)] = &[("Engine", "valkey"), ("MajorEngineVersion", "9")];

/// One row of the create / round-trip table.
#[derive(Debug, Clone, Copy)]
struct IntegrationCase {
    /// Names the row in every assertion.
    case: &'static str,
    /// The `ServerlessCacheName` the request sends.
    name: &'static str,
    /// The pairs sent beside it.
    extra: &'static [(&'static str, &'static str)],
    /// The `IntegrationType`s the stored cache must report, in document order.
    expected: &'static [&'static str],
}

/// The axis: how many integrations the request names, the position they arrive
/// at, whether one type is named twice, and whether any is named at all.
const CASES: &[IntegrationCase] = &[
    IntegrationCase {
        case: "no integration at all",
        name: "plain-cache",
        extra: INTEGRATION_ENGINE,
        expected: &[],
    },
    IntegrationCase {
        case: "one integration",
        name: "Integrated-Cache",
        extra: &[
            ("Engine", "valkey"),
            ("MajorEngineVersion", "9"),
            ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
        ],
        expected: &[DYNAMODB],
    },
    IntegrationCase {
        case: "one integration beside the other members of a create",
        name: "configured-cache",
        extra: &[
            ("Engine", "valkey"),
            ("MajorEngineVersion", "9"),
            ("ConnectionType", "public"),
            ("Description", "the integrated cache"),
            ("SnapshotRetentionLimit", "7"),
            ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
        ],
        expected: &[DYNAMODB],
    },
    IntegrationCase {
        // The type is the integration's identity on the cache — one status per
        // type — so a list naming it twice configures the ONE integration.
        case: "the same integration named twice",
        name: "twice-cache",
        extra: &[
            ("Engine", "valkey"),
            ("MajorEngineVersion", "9"),
            ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
            ("Integrations.IntegrationRequest.2.IntegrationType", DYNAMODB),
        ],
        expected: &[DYNAMODB],
    },
];

/// Create one serverless cache.
async fn create_cache(emulator: &Emulator, name: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("ServerlessCacheName", name)]);
    pairs.extend(params(extra));
    emulator.call("CreateServerlessCache", &pairs).await
}

/// Describe one serverless cache by name.
async fn describe_cache(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", name)]),
        )
        .await
}

/// Assert `reply` reports `case`'s integrations, each with `status`.
fn assert_reports_integrations(case: &IntegrationCase, where_from: &str, reply: &Reply, status: &str) {
    let name = case.case;
    let types = reply.all("Integrations/Integration/IntegrationType");
    assert_eq!(
        types, case.expected,
        "case {name} ({where_from}): expected IntegrationTypes {:?}, got {types:?}",
        case.expected
    );
    let statuses = reply.all("Integrations/Integration/Status");
    let expected_statuses: Vec<&str> = case.expected.iter().map(|_| status).collect();
    assert_eq!(
        statuses, expected_statuses,
        "case {name} ({where_from}): expected integration statuses {expected_statuses:?}, got \
         {statuses:?}"
    );
    // A cache carrying none reports no list at all, rather than an empty one: the
    // member is absent, which is what "no integrations are enabled" looks like on
    // the wire.
    if case.expected.is_empty() {
        assert!(
            !carries_element(&reply.body, "Integrations"),
            "case {name} ({where_from}): a cache with no integration must report no Integrations \
             element, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_created_cache_reports_the_integrations_it_was_created_with() {
    for case in CASES {
        let emulator = Emulator::new();
        // The integration the create asks for is brought up after the call
        // answers, so the change the clock owes it is held out of reach: what
        // this row reads is the handler's own answer rather than a race with
        // the span.
        let created = create_cache(&emulator, case.name, case.extra).await;
        assert_eq!(
            created.status, 200,
            "case {}: CreateServerlessCache must answer 200, got {} {}",
            case.case, created.status, created.body
        );
        hold_pending_changes(&emulator);
        // "Enabling": the create answers with the integration being brought up,
        // alongside the cache itself still being created.
        assert_reports_integrations(case, "create", &created, "enabling");

        // The read-back sees the SAME integrations, off the one stored record.
        let described = describe_cache(&emulator, case.name).await;
        assert_eq!(
            described.status, 200,
            "case {}: DescribeServerlessCaches must answer 200, got {} {}",
            case.case, described.status, described.body
        );
        assert_reports_integrations(case, "describe", &described, "enabling");

        // And once the change the clock owes falls due, every integration the
        // cache carries reports `enabled` — on the read, without anything
        // calling for it.
        release_pending_changes(&emulator);
        let settled = poll_until(
            &emulator,
            REGION,
            case.case,
            "the cache becoming available",
            "DescribeServerlessCaches",
            &params(&[("ServerlessCacheName", case.name)]),
            |reply| reply.text("ServerlessCaches/member/Status") == Some("available"),
        )
        .await;
        assert_reports_integrations(case, "settled describe", &settled, "enabled");
    }
}

/// One refusal row: the case, the whole request, and the HTTP status, `awsQuery`
/// code and sentence it is answered with.
type RefusedCreate = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
    &'static str,
);

#[tokio::test]
async fn an_integration_is_refused_on_a_pair_that_cannot_carry_it() {
    // The axis: the engine and the version line the request would leave the cache
    // on — another engine, the integration's own engine below the version it
    // needs, and the engine that has no version line of that shape at all.
    let cases: &[RefusedCreate] = &[
        (
            "another engine on its own current line",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "redis"),
                ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
            ],
            400,
            "InvalidParameterCombination",
            "Integration dynamodb requires valkey 9 or above, not redis 7.",
        ),
        (
            "the integration's engine below the version it needs",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "valkey"),
                ("MajorEngineVersion", "8"),
                ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
            ],
            400,
            "InvalidParameterCombination",
            "Integration dynamodb requires valkey 9 or above, not valkey 8.",
        ),
        (
            "an engine whose version line is not a major number at all",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "memcached"),
                ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
            ],
            400,
            "InvalidParameterCombination",
            "Integration dynamodb requires valkey 9 or above, not memcached 1.6.",
        ),
        (
            // A type outside the modelled `IntegrationType` enum is refused
            // while the request is validated, before any handler reads it — so
            // no cache is created for it either. The sentence is the validator's
            // own, which is why this row asserts the status and the code and
            // leaves the wording to it.
            "a type the enum does not hold",
            &[
                ("ServerlessCacheName", "refused"),
                ("Engine", "valkey"),
                ("MajorEngineVersion", "9"),
                (
                    "Integrations.IntegrationRequest.1.IntegrationType",
                    "postgresql",
                ),
            ],
            400,
            "ValidationError",
            "",
        ),
    ];

    for (case, pairs, status, code, sentence) in cases {
        let emulator = Emulator::new();
        let refused = emulator.call("CreateServerlessCache", &params(pairs)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        if !sentence.is_empty() {
            assert_eq!(
                refused.error_message(),
                *sentence,
                "case {case}: expected message {sentence:?}, got {:?}",
                refused.error_message()
            );
        }
        // Nothing was stored, so nothing is reported.
        let listed = emulator
            .call("DescribeServerlessCaches", &params(&[]))
            .await;
        assert!(
            listed.all("ServerlessCacheName").is_empty(),
            "case {case}: a refused create must store nothing, got {}",
            listed.body
        );
    }
}

/// One refused modify row: the case, the pairs the modify names beside the
/// cache's name, and the sentence it is refused with.
type RefusedModify = (&'static str, &'static [(&'static str, &'static str)], &'static str);

#[tokio::test]
async fn a_modify_cannot_move_a_cache_carrying_an_integration_off_the_pair_that_carries_it() {
    let emulator = Emulator::new();
    let mut created = params(&[("ServerlessCacheName", "moved-cache")]);
    created.extend(params(&[
        ("Engine", "valkey"),
        ("MajorEngineVersion", "9"),
        ("Integrations.IntegrationRequest.1.IntegrationType", DYNAMODB),
    ]));
    let created = emulator.call("CreateServerlessCache", &created).await;
    assert_eq!(
        created.status, 200,
        "the cache must be created, got {} {}",
        created.status, created.body
    );
    // A modify is served on a cache that has been provisioned, so the change the
    // clock owes the create is carried through first.
    release_pending_changes(&emulator);
    poll_until(
        &emulator,
        REGION,
        "moved-cache",
        "the cache becoming available",
        "DescribeServerlessCaches",
        &params(&[("ServerlessCacheName", "moved-cache")]),
        |reply| reply.text("ServerlessCaches/member/Status") == Some("available"),
    )
    .await;

    // The axis: the member the modify names to move the cache off the pair — the
    // engine, the version line, and both together.
    let cases: &[RefusedModify] = &[
        (
            "onto another engine",
            &[("Engine", "redis")],
            "Integration dynamodb requires valkey 9 or above, not redis 9.",
        ),
        (
            "down to an earlier version line",
            &[("MajorEngineVersion", "8")],
            "Integration dynamodb requires valkey 9 or above, not valkey 8.",
        ),
        (
            "onto another engine and its own line",
            &[("Engine", "memcached"), ("MajorEngineVersion", "1.6")],
            "Integration dynamodb requires valkey 9 or above, not memcached 1.6.",
        ),
    ];

    for (case, extra, sentence) in cases {
        let mut pairs = params(&[("ServerlessCacheName", "moved-cache")]);
        pairs.extend(params(extra));
        let refused = emulator.call("ModifyServerlessCache", &pairs).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "InvalidParameterCombination"),
            "case {case}: expected 400 InvalidParameterCombination, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            *sentence,
            "case {case}: expected message {sentence:?}, got {:?}",
            refused.error_message()
        );
        // The refused modify changed nothing: the cache still runs the pair that
        // carries its integration, and still reports it.
        let described = describe_cache(&emulator, "moved-cache").await;
        for (member, expected) in [
            ("ServerlessCaches/member/Engine", "valkey"),
            ("ServerlessCaches/member/MajorEngineVersion", "9"),
            ("Integrations/Integration/IntegrationType", DYNAMODB),
            ("Integrations/Integration/Status", "enabled"),
        ] {
            assert_eq!(
                described.text(member),
                Some(expected),
                "case {case}: expected {member} {expected:?} after the refusal, got {:?}",
                described.text(member)
            );
        }
    }

    // And a modify that leaves the cache on a pair that CAN carry the integration
    // is served, so the rule above is the pair rather than the member being named
    // at all.
    let mut served = params(&[("ServerlessCacheName", "moved-cache")]);
    served.extend(params(&[("Engine", "valkey"), ("MajorEngineVersion", "9")]));
    let served = emulator.call("ModifyServerlessCache", &served).await;
    assert_eq!(
        served.status, 200,
        "a modify that keeps the pair must be served, got {} {}",
        served.status, served.body
    );
    assert_eq!(
        served.text("Integrations/Integration/IntegrationType"),
        Some(DYNAMODB),
        "the served modify must report the cache's integration, got {}",
        served.body
    );
}
