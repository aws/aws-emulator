// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamPrefixListResolver`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * deleting an IPAM prefix list resolver WRITES that deletion against exactly the resolver
//!   the request names: it is answered, and read back by
//!   `DescribeIpamPrefixListResolvers`, in the transient `delete-in-progress` state with every
//!   other member of the stored resolver unchanged, and the CLOCK — not a later call — moves it
//!   on to `delete-complete`;
//! * "before deleting a resolver, you must first delete all resolver targets associated with
//!   it" (`DeleteIpamPrefixListResolver`): a resolver that still holds a target is refused with
//!   EC2's code for a delete the resource's dependencies forbid, and nothing is written;
//! * an id the account holds no resolver for is refused with this resource's own `NotFound`
//!   code and the one fixed sentence this family words that refusal with, and a resolver whose
//!   deletion has already been asked for with `IncorrectState` naming the state it is in —
//!   neither writes anything;
//! * `DryRun` answers the permission question with `DryRunOperation` and writes nothing,
//!   whether or not the resolver it names exists.
//!
//! Each table row is a distinct input along what those rules generalise over: which address
//! family the resolver holds, which optional members the create carried, how many CIDR
//! selection rules and tags it kept, how much of the resolver's life the clock has already run
//! when the delete arrives, which id the request names and in what way it does not resolve.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use ec2::ops::types::IPAM_PREFIX_LIST_RESOLVER_STATE_VALUES;
use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEPENDENCY_VIOLATION, DRY_RUN_OPERATION, INCORRECT_STATE,
    INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND, IPAM_PREFIX_LIST_RESOLVER_REFUSAL_NAME,
    IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE,
    IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS, IPAM_STATE_CREATE_COMPLETE,
    IPAM_STATE_CREATE_IN_PROGRESS, PREFIX_LIST_ADDRESS_FAMILY_IPV4,
    RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The action under test.
const DELETE: &str = "DeleteIpamPrefixListResolver";

/// The action that creates the resolvers it deletes.
const CREATE: &str = "CreateIpamPrefixListResolver";

/// The action that reads them back.
const DESCRIBE: &str = "DescribeIpamPrefixListResolvers";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed `IpamPrefixListResolverId` the account holds no resolver for.
///
/// Spelled from the top of the hexadecimal range rather than the bottom: this store mints ids
/// from a monotonic counter (`next_id`), so an id this high cannot collide with one a created
/// resolver was given however many ids a run has minted.
const UNKNOWN_RESOLVER_ID: &str = "ipam-plr-0ffffffffffffffff";

/// One rule type of the modelled catalog
/// (`ec2::ops::types::IPAM_PREFIX_LIST_RESOLVER_RULE_TYPE_VALUES`), used to give a deleted
/// resolver CIDR selection rules of its own.
const RULE_TYPE_STATIC_CIDR: &str = "static-cidr";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and the
/// engine its store lives in.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for [`REGION`].
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The IPAM a suite creates through the operation that owns it: the id a resolver create names
/// it by, and one of the scopes it came up with, which a CIDR selection rule may select from.
struct CreatedIpam {
    id: String,
    scope: String,
}

/// Create one IPAM through `CreateIpam` and answer what a request names it by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let read = |name: &str| -> String {
        element(&body, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the created IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        scope: read("privateDefaultScopeId"),
    }
}

/// One resolver as a response reports it: every member read off the wire except the state, which
/// is what a delete MOVES and so is asserted on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedResolver {
    resolver_id: Option<String>,
    resolver_arn: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    owner_id: Option<String>,
    description: Option<String>,
    address_family: Option<String>,
    tags: Vec<(String, String)>,
}

/// The resolver element of `body` as [`ReportedResolver`] and the state it reports: both a
/// create, a delete and a describe carry the same `IpamPrefixListResolver` shape, so one reader
/// serves all three.
fn read_resolver(case: &str, body: &str) -> (ReportedResolver, Option<String>) {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPrefixListResolverId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no resolver in {body}"));
    let tags = child(node, "tagSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element())
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let reported = ReportedResolver {
        resolver_id: child_text(node, "ipamPrefixListResolverId"),
        resolver_arn: child_text(node, "ipamPrefixListResolverArn"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        owner_id: child_text(node, "ownerId"),
        description: child_text(node, "description"),
        address_family: child_text(node, "addressFamily"),
        tags,
    };
    let state = child_text(node, "state");
    (reported, state)
}

/// The resolver `DescribeIpamPrefixListResolvers` reports for `resolver_id`, and the state it
/// reports it in.
async fn describe_one(
    case: &str,
    router: &MultiServiceRouter,
    resolver_id: &str,
) -> (ReportedResolver, Option<String>) {
    let body = ok(
        case,
        router,
        DESCRIBE,
        &indexed("IpamPrefixListResolverId", &[resolver_id]),
    )
    .await;
    read_resolver(case, &body)
}

/// The state `DescribeIpamPrefixListResolvers` reports for `resolver_id` right now.
async fn described_state(case: &str, router: &MultiServiceRouter, resolver_id: &str) -> String {
    describe_one(case, router, resolver_id)
        .await
        .1
        .unwrap_or_default()
}

/// The state `DescribeIpamPrefixListResolvers` reports for `resolver_id` once it is `wanted`:
/// the state is looked at AGAIN in a loop until it has moved or a deadline far past
/// [`TRANSITION_DELAY`] passes, so the wait is for the CONDITION rather than for a span of the
/// clock.
async fn described_state_once(
    case: &str,
    router: &MultiServiceRouter,
    resolver_id: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = described_state(case, router, resolver_id).await;
        if state == wanted || Instant::now() >= deadline {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The state the stored resolver `resolver_id` is in right now, read straight out of the store;
/// `None` when the account holds no resolver under that id.
fn stored_state(engine: &engine::InMemoryEngine, resolver_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_prefix_list_resolvers
        .get(resolver_id)
        .and_then(|resolver| resolver.state.clone())
}

/// Whether the stored resolver `resolver_id` is in one of the two delete states — what a
/// refused delete must NOT have written.
///
/// The exact state a resolver was in before a refused delete is not what a row asserts: the
/// CLOCK moves a created resolver from `create-in-progress` to `create-complete` on its own,
/// whether or not anybody calls, so pinning that word would make a row fail for a move the
/// clock owed rather than for a write the refusal made.
fn is_being_deleted(engine: &engine::InMemoryEngine, resolver_id: &str) -> bool {
    stored_state(engine, resolver_id).is_some_and(|state| {
        state == IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS
            || state == IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE
    })
}

/// The ids of every resolver the store holds — what proves a delete removed none of them and a
/// refusal added none.
fn stored_resolver_ids(engine: &engine::InMemoryEngine) -> BTreeSet<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_prefix_list_resolvers
        .keys()
        .cloned()
        .collect()
}

/// The form fields for one `ipam-prefix-list-resolver` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    if tags.is_empty() {
        return Vec::new();
    }
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// The form fields for `count` static-CIDR selection rules, as ec2Query flattens the `Rules`
/// list into indexed `Rule.N` keys.
fn rule_params(count: usize) -> Vec<Param> {
    let mut params = Vec::new();
    for position in 1..=count {
        params.push(param(&format!("Rule.{position}.RuleType"), RULE_TYPE_STATIC_CIDR));
        params.push(param(
            &format!("Rule.{position}.StaticCidr"),
            &format!("10.{position}.0.0/16"),
        ));
    }
    params
}

/// Create one resolver in the IPAM `ipam` names and answer its id.
async fn create_resolver(
    case: &str,
    router: &MultiServiceRouter,
    ipam: &CreatedIpam,
    family: &str,
    description: Option<&str>,
    tags: &[(&str, &str)],
    rules: usize,
) -> String {
    let mut params = vec![
        param("IpamId", &ipam.id),
        param("AddressFamily", family),
    ];
    if let Some(description) = description {
        params.push(param("Description", description));
    }
    params.extend(tag_params(tags));
    params.extend(rule_params(rules));
    // A rule naming the IPAM's own scope, so a row with rules also covers a rule that selects
    // from a scope the account holds rather than only a static CIDR.
    if rules > 0 {
        params.push(param("Rule.1.IpamScopeId", &ipam.scope));
    }
    let body = ok(case, router, CREATE, &params).await;
    read_resolver(case, &body)
        .0
        .resolver_id
        .unwrap_or_else(|| panic!("case {case:?}: no resolver id in {body}"))
}

/// Create one customer-managed prefix list through the operation that owns it, and answer the
/// `PrefixListId` it minted.
async fn create_prefix_list(case: &str, router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", name),
            param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
            param("MaxEntries", "5"),
        ],
    )
    .await;
    element(&body, "prefixListId")
        .unwrap_or_else(|| panic!("case {case:?}: no <prefixListId> in {body}"))
}

// ---------------------------------------------------------------------------
// The words the suite pins
// ---------------------------------------------------------------------------

/// The state words this suite asserts on are the model's own: each is a member of the catalog
/// the model publishes for `IpamPrefixListResolverState`, so a row cannot pin a word the API
/// does not have.
#[test]
fn the_asserted_state_words_are_the_models_own() {
    for (case, word) in [
        ("a resolver being created", IPAM_STATE_CREATE_IN_PROGRESS),
        ("a resolver in service", IPAM_STATE_CREATE_COMPLETE),
        (
            "a resolver being deleted",
            IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS,
        ),
        (
            "a resolver whose deletion is done",
            IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE,
        ),
    ] {
        assert!(
            IPAM_PREFIX_LIST_RESOLVER_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, which \
             carries {IPAM_PREFIX_LIST_RESOLVER_STATE_VALUES:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a delete answers and writes
// ---------------------------------------------------------------------------

/// One resolver to delete: a distinct input along what the delete generalises over.
struct Row {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The address family the resolver holds.
    family: &'static str,
    /// The description the create carried, if any.
    description: Option<&'static str>,
    /// The tags the create carried.
    tags: &'static [(&'static str, &'static str)],
    /// How many CIDR selection rules the resolver was created with.
    rules: usize,
    /// Whether the clock has already finished the resolver's creation when the delete arrives.
    settled: bool,
}

/// The resolvers the delete is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bare IPv4 resolver deleted while it is still being created",
            family: ADDRESS_FAMILY_IPV4,
            description: None,
            tags: &[],
            rules: 0,
            settled: false,
        },
        Row {
            case: "a described and tagged IPv6 resolver with one rule, deleted once its \
                   creation has settled",
            family: ADDRESS_FAMILY_IPV6,
            description: Some("space being retired"),
            tags: &[("Name", "retired"), ("Team", "network")],
            rules: 1,
            settled: true,
        },
        Row {
            case: "a described IPv4 resolver with two rules and one tag",
            family: ADDRESS_FAMILY_IPV4,
            description: Some("two rules"),
            tags: &[("Name", "two-rules")],
            rules: 2,
            settled: false,
        },
        Row {
            case: "a bare IPv6 resolver deleted once its creation has settled",
            family: ADDRESS_FAMILY_IPV6,
            description: None,
            tags: &[],
            rules: 1,
            settled: true,
        },
    ]
}

/// The rule: a delete answers the resolver it named as the store then holds it — the transient
/// `delete-in-progress` state with every other member unchanged — the describe reads that back,
/// the CLOCK moves it on to `delete-complete`, and no other resolver of the account is touched.
#[tokio::test]
async fn deleting_a_resolver_reports_it_deleting_and_reaches_delete_complete() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let resolver_id = create_resolver(
            case,
            &router,
            &ipam,
            row.family,
            row.description,
            row.tags,
            row.rules,
        )
        .await;
        // The control resolver every row keeps beside the one it deletes: each delete is
        // checked to touch only the resolver it names.
        let control_id =
            create_resolver(case, &router, &ipam, row.family, None, &[], 0).await;

        let expected_state_before = if row.settled {
            described_state_once(case, &router, &resolver_id, IPAM_STATE_CREATE_COMPLETE).await
        } else {
            described_state(case, &router, &resolver_id).await
        };
        if row.settled {
            assert_eq!(
                expected_state_before, IPAM_STATE_CREATE_COMPLETE,
                "case {case:?}: the clock settles a created resolver before the delete"
            );
        }
        let (before, _) = describe_one(case, &router, &resolver_id).await;

        let body = ok(case, &router, DELETE, &[param("IpamPrefixListResolverId", &resolver_id)]).await;
        let (deleted, state) = read_resolver(case, &body);
        assert_eq!(
            state.as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a deletion is answered while it is still under way, got {body}"
        );
        // Everything but the state is the resolver the store already held: the delete reports a
        // resource, it does not rebuild one.
        assert_eq!(
            deleted, before,
            "case {case:?}: expected the stored resolver's own members back, got {body}"
        );
        assert_eq!(
            deleted.resolver_id.as_deref(),
            Some(resolver_id.as_str()),
            "case {case:?}: expected the deleted resolver's id, got {body}"
        );
        assert_eq!(
            deleted.address_family.as_deref(),
            Some(row.family),
            "case {case:?}: expected the family the resolver holds, got {body}"
        );
        assert_eq!(
            deleted.description.as_deref(),
            row.description,
            "case {case:?}: expected the description the create carried, got {body}"
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            deleted.tags, expected_tags,
            "case {case:?}: expected the tags the create carried, got {body}"
        );
        assert_eq!(
            deleted.owner_id.as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the account that owns the resolver, got {body}"
        );

        // The write the response reported is in the store, and the describe reads it back.
        assert_eq!(
            stored_state(&engine, &resolver_id).as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the store must hold the deletion the response reported"
        );
        let (described, described_state) = describe_one(case, &router, &resolver_id).await;
        assert_eq!(
            described_state.as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the describe reports the deletion a caller asked for"
        );
        assert_eq!(
            described, before,
            "case {case:?}: the describe reports the same resolver the delete answered"
        );
        assert_ne!(
            stored_state(&engine, &control_id).as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: no other resolver of the account may be deleted"
        );

        // The move the clock owed falls due on its own: the state is looked at again until it
        // holds, so nothing here depends on how fast the host runs.
        assert_eq!(
            described_state_once(
                case,
                &router,
                &resolver_id,
                IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE,
            )
            .await,
            IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE,
            "case {case:?}: the clock owed this resolver its terminal state"
        );
        // A deleted resolver still has a state to report, so the record stays in the store and
        // the delete removed nothing.
        assert_eq!(
            stored_resolver_ids(&engine),
            BTreeSet::from([resolver_id.clone(), control_id.clone()]),
            "case {case:?}: the account still holds both resolvers"
        );
    }
}

// ---------------------------------------------------------------------------
// What is still associated with the resolver
// ---------------------------------------------------------------------------

/// The rule: "before deleting a resolver, you must first delete all resolver targets associated
/// with it" (`DeleteIpamPrefixListResolver`) — a resolver that still holds a target is refused
/// with `DependencyViolation` and nothing is written, while a resolver of the same account that
/// holds none deletes.
#[tokio::test]
async fn a_resolver_that_still_has_a_target_is_refused() {
    for (case, targets) in [
        ("one target associated with the resolver", 1usize),
        ("two targets associated with the resolver", 2),
    ] {
        let (router, engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let resolver_id =
            create_resolver(case, &router, &ipam, ADDRESS_FAMILY_IPV4, None, &[], 0).await;
        // A second resolver of the same account, with no target of its own: what proves the
        // refusal is about the targets this resolver holds rather than about any target.
        let free_id = create_resolver(case, &router, &ipam, ADDRESS_FAMILY_IPV4, None, &[], 0).await;
        for nth in 0..targets {
            let prefix_list = create_prefix_list(case, &router, &format!("list-{nth}")).await;
            ok(
                case,
                &router,
                "CreateIpamPrefixListResolverTarget",
                &[
                    param("IpamPrefixListResolverId", &resolver_id),
                    param("PrefixListId", &prefix_list),
                    param("PrefixListRegion", REGION),
                    param("TrackLatestVersion", "true"),
                ],
            )
            .await;
        }

        let state_before = stored_state(&engine, &resolver_id);
        assert!(
            state_before.is_some(),
            "case {case:?}: the account holds the resolver the delete names"
        );
        let (status, body) = invoke(
            &router,
            DELETE,
            &[param("IpamPrefixListResolverId", &resolver_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DEPENDENCY_VIOLATION,
            "case {case:?}: expected {DEPENDENCY_VIOLATION}, got body {body}"
        );
        assert!(
            error_message(&body).contains(&resolver_id),
            "case {case:?}: the refusal names the resolver it is not valid for, got body {body}"
        );
        assert!(
            !is_being_deleted(&engine, &resolver_id),
            "case {case:?}: a refused delete must not have started one, the store holds \
             {state:?}",
            state = stored_state(&engine, &resolver_id)
        );

        // The resolver that holds no target is deletable, which is what makes the refusal above
        // about the dependency rather than about deleting at all.
        let body = ok(case, &router, DELETE, &[param("IpamPrefixListResolverId", &free_id)]).await;
        assert_eq!(
            read_resolver(case, &body).1.as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a resolver with no target of its own deletes, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the delete refuses
// ---------------------------------------------------------------------------

/// The rule: an id the account holds no resolver of this kind for is refused with this
/// resource's own `NotFound` code and the one fixed sentence this family words that refusal
/// with — whatever shape the id has — and nothing is written.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_this_resources_own_code() {
    for (case, requested) in [
        (
            "a well-formed id the account holds no resolver for",
            UNKNOWN_RESOLVER_ID.to_string(),
        ),
        (
            "a well-formed short-form id the account holds no resolver for",
            "ipam-plr-0fffffff".to_string(),
        ),
        ("an id of another resource kind", "ipam-0ffffffffffffffff".to_string()),
        ("not an id at all", "my-resolver".to_string()),
    ] {
        let (router, engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let kept = create_resolver(case, &router, &ipam, ADDRESS_FAMILY_IPV4, None, &[], 0).await;

        let (status, body) = invoke(
            &router,
            DELETE,
            &[param("IpamPrefixListResolverId", &requested)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND}, got body \
             {body}"
        );
        // The sentence is FIXED for this family: it names the resource and no id.
        assert_eq!(
            error_message(&body),
            format!("The specified {IPAM_PREFIX_LIST_RESOLVER_REFUSAL_NAME} does not exist."),
            "case {case:?}: expected this family's one refusal sentence, got body {body}"
        );
        assert!(
            !is_being_deleted(&engine, &kept),
            "case {case:?}: a refused request must not have deleted the account's resolver, \
             the store holds {state:?}",
            state = stored_state(&engine, &kept)
        );
        assert_eq!(
            stored_resolver_ids(&engine),
            BTreeSet::from([kept.clone()]),
            "case {case:?}: a refused request stores nothing and removes nothing"
        );
    }
}

/// The rule: a resolver whose deletion has already been asked for has nothing left to delete, so
/// a second delete is refused with `IncorrectState` naming the state it is actually in — while
/// the deletion is under way and once the clock has finished it.
#[tokio::test]
async fn a_second_delete_is_refused_as_an_incorrect_state() {
    for (case, wait) in [
        ("while the deletion is still under way", false),
        ("once the clock has finished the deletion", true),
    ] {
        let (router, engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let resolver_id =
            create_resolver(case, &router, &ipam, ADDRESS_FAMILY_IPV4, None, &[], 0).await;
        ok(
            case,
            &router,
            DELETE,
            &[param("IpamPrefixListResolverId", &resolver_id)],
        )
        .await;

        let expected_state = if wait {
            described_state_once(
                case,
                &router,
                &resolver_id,
                IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE,
            )
            .await
        } else {
            IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS.to_string()
        };

        let (status, body) = invoke(
            &router,
            DELETE,
            &[param("IpamPrefixListResolverId", &resolver_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "{IPAM_PREFIX_LIST_RESOLVER_REFUSAL_NAME} '{resolver_id}' is in the \
                 '{expected_state}' state."
            ),
            "case {case:?}: expected the state the resolver is in to be named, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &resolver_id).as_deref(),
            Some(expected_state.as_str()),
            "case {case:?}: the refused repeat must not have restarted the deletion"
        );
    }
}

/// The rule: `DryRun` is a question about the CALLER, answered with `DryRunOperation` before
/// anything is resolved or written — so it answers the same whether or not the resolver the
/// request names exists, and the account's resolvers are left exactly as they were.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    for (case, requested) in [
        ("a resolver the account holds", None),
        (
            "an id the account holds no resolver for",
            Some(UNKNOWN_RESOLVER_ID),
        ),
    ] {
        let (router, engine) = emulator();
        let ipam = create_ipam(case, &router).await;
        let resolver_id =
            create_resolver(case, &router, &ipam, ADDRESS_FAMILY_IPV4, None, &[], 0).await;

        let (status, body) = invoke(
            &router,
            DELETE,
            &[
                param(
                    "IpamPrefixListResolverId",
                    requested.unwrap_or(resolver_id.as_str()),
                ),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got body {body}"
        );
        assert!(
            !is_being_deleted(&engine, &resolver_id),
            "case {case:?}: a dry run must write nothing, the store holds {state:?}",
            state = stored_state(&engine, &resolver_id)
        );

        // The same request without the flag is what does the work, which is what makes the
        // answer above the permitted side of the probe.
        let body = ok(
            case,
            &router,
            DELETE,
            &[param("IpamPrefixListResolverId", &resolver_id)],
        )
        .await;
        assert_eq!(
            read_resolver(case, &body).1.as_deref(),
            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the delete the dry run stood for succeeds, got {body}"
        );
    }
}
