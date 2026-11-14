// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIpamInternetRegistryAssociations`, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * A paginated EC2 describe REPORTS what the account holds: the whole store by default
//!   (including nothing at all, when the account holds no association yet), narrowed by the
//!   resource ids the request names and by every filter the operation resolves — one filter
//!   per member of the reported resource, spelled in kebab-case, values joined with OR and
//!   filters with AND — answered at most `MaxResults` rows at a time in a stable id order
//!   with a `NextToken` naming where the next page starts, and it changes nothing a later
//!   caller sees beyond the transitions the clock had already made due.
//! * An id outside the form this service ISSUES for the resource a member names is refused
//!   with that resource's own `Invalid<Kind>.Malformed` before any lookup; an id of that form
//!   that names nothing the account holds is refused with its `Invalid<Kind>.NotFound`; a
//!   filter name, a page token or a request-controlled count the operation will not serve is
//!   refused rather than answered as "no results"; and `DryRun` answers the permission
//!   question and reads nothing.
//!
//! Each table row is a distinct input along what those rules generalise over: which Regional
//! Internet Registry the association is with, the IPAM and Region it belongs to, whether it
//! carries a description or a state message, how many tags it has, which state it sits in,
//! which member a filter selects on, how large a page is asked for, and which part of an id
//! is outside the issued form.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::types::{
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES, IpamInternetRegistryAssociation, RIR_VALUES,
    Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
    RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, emulator, error_code, error_message, indexed};

/// The operation under test.
const ACTION: &str = "DescribeIpamInternetRegistryAssociations";

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The IPAM the seeded associations belong to unless a row names another.
const IPAM_ID: &str = "ipam-0123456789abcdef0";

/// The `IpamInternetRegistryAssociationState` catalog's `enable-complete` member: an
/// association in service, which [`IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`] is
/// asserted to carry below.
const STATE_ENABLE_COMPLETE: &str = "enable-complete";

/// That catalog's `disable-complete` member.
const STATE_DISABLE_COMPLETE: &str = "disable-complete";

/// That catalog's `create-failed` member: an association whose setup never finished, the one
/// state a seeded row carries a state message in.
const STATE_CREATE_FAILED: &str = "create-failed";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field of an ec2Query request body.
type Param = (String, String);

/// One refusal row: the case, the request that carries it and the code EC2 answers.
type RefusalCase = (&'static str, Vec<Param>, &'static str);

/// One `(name, value)` form field.
fn param(name: &str, value: &str) -> Param {
    (name.to_string(), value.to_string())
}

/// One describe call signed for the Region above, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// One describe call required to succeed, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// The association ids a describe body reports, in the order it reported them.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "ipamInternetRegistryAssociationId")
}

/// The association ids a describe body reports, as a set.
fn reported_id_set(body: &str) -> BTreeSet<String> {
    reported_ids(body).into_iter().collect()
}

/// What one seeded association is made of — the inputs the describe generalises over.
struct Seed {
    /// The IPAM it belongs to.
    ipam_id: &'static str,
    /// The Region of that IPAM.
    region: &'static str,
    /// The Regional Internet Registry it is with, a member of the modelled `Rir` catalog.
    rir: &'static str,
    /// Its organization handle at that registry.
    organization_handle: &'static str,
    /// Its description, when it carries one.
    description: Option<&'static str>,
    /// The state it sits in.
    state: &'static str,
    /// The message about that state, when it carries one.
    state_message: Option<&'static str>,
    /// The tags assigned to it.
    tags: &'static [(&'static str, &'static str)],
}

/// Seed one internet registry association and answer the id this service minted for it.
fn seed_association(engine: &engine::InMemoryEngine, seed: &Seed) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let association_id = guard
        .insert_ipam_internet_registry_association(IpamInternetRegistryAssociation {
            ipam_id: Some(seed.ipam_id.to_string()),
            ipam_region: Some(seed.region.to_string()),
            rir: Some(seed.rir.to_string()),
            organization_handle: Some(seed.organization_handle.to_string()),
            description: seed.description.map(str::to_string),
            state: Some(seed.state.to_string()),
            state_message: seed.state_message.map(str::to_string),
            tags: Some(
                seed.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
            ..Default::default()
        })
        .expect("the store holds the seeded association");
    assert!(
        association_id.starts_with(&format!("{IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX}-")),
        "the minted association id {association_id:?} must carry the issued prefix \
         {IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX:?}"
    );
    association_id
}

/// The associations the describe is asserted over: every Regional Internet Registry the model
/// publishes, in four Regions and under two IPAMs, with and without a description, with no
/// tags, one and three, with and without a state message, and from three states an
/// association can sit in.
fn rows() -> Vec<(&'static str, Seed)> {
    vec![
        (
            "an arin association with no tags, in the Region the request is signed for",
            Seed {
                ipam_id: IPAM_ID,
                region: REGION,
                rir: "arin",
                organization_handle: "EXAMPLE-ORG",
                description: None,
                state: STATE_ENABLE_COMPLETE,
                state_message: None,
                tags: &[],
            },
        ),
        (
            "a ripe association with a description and one tag, in another Region",
            Seed {
                ipam_id: IPAM_ID,
                region: "eu-west-2",
                rir: "ripe",
                organization_handle: "ORG-EX1-RIPE",
                description: Some("delegated rpki for the edge prefixes"),
                state: STATE_ENABLE_COMPLETE,
                state_message: None,
                tags: &[("Name", "edge")],
            },
        ),
        (
            "an apnic association with three tags, under another IPAM",
            Seed {
                ipam_id: "ipam-00000000000000abc",
                region: "ap-southeast-1",
                rir: "apnic",
                organization_handle: "A12345678",
                description: Some("apac space"),
                state: STATE_DISABLE_COMPLETE,
                state_message: None,
                tags: &[("Name", "apac"), ("Team", "network"), ("Env", "prod")],
            },
        ),
        (
            "a lacnic association whose creation failed, carrying a state message",
            Seed {
                ipam_id: IPAM_ID,
                region: "sa-east-1",
                rir: "lacnic",
                organization_handle: "LACNIC-EX",
                description: None,
                state: STATE_CREATE_FAILED,
                state_message: Some("the registry rejected the organization handle"),
                tags: &[],
            },
        ),
        (
            "a nicbr association with a description and no tags",
            Seed {
                ipam_id: IPAM_ID,
                region: "sa-east-1",
                rir: "nicbr",
                organization_handle: "NICBR-EX",
                description: Some("brazil space"),
                state: STATE_ENABLE_COMPLETE,
                state_message: None,
                tags: &[("Team", "network")],
            },
        ),
    ]
}

/// The words this suite asserts on are the model's own: each state is a member of the catalog
/// the model publishes for its enum and each registry a member of `Rir`, so a row cannot pin
/// a word the API does not have — and every registry the catalog carries has a row.
#[test]
fn the_asserted_words_are_the_models_own() {
    for (case, word) in [
        ("an association in service", STATE_ENABLE_COMPLETE),
        ("an association that has been disabled", STATE_DISABLE_COMPLETE),
        ("an association whose creation failed", STATE_CREATE_FAILED),
        (
            "an association being deleted",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
        ),
        (
            "an association whose deletion is done",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
        ),
    ] {
        assert!(
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled state catalog, \
             which carries {IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES:?}"
        );
    }
    for (case, seed) in rows() {
        assert!(
            RIR_VALUES.contains(&seed.rir),
            "case {case:?}: expected the registry {rir:?} to be a member of the modelled Rir \
             catalog, which carries {RIR_VALUES:?}",
            rir = seed.rir
        );
    }
    for rir in RIR_VALUES {
        assert!(
            rows().iter().any(|(_, seed)| seed.rir == *rir),
            "expected a row for the registry {rir:?}, which the modelled Rir catalog carries"
        );
    }
}

/// The rule: the describe reports what the account holds — nothing at all before anything is
/// created, then every stored association with the members the store holds for it, narrowed
/// to the ids the request names — and reporting changes nothing a later caller sees.
#[tokio::test]
async fn describing_reports_the_store_and_narrows_by_the_ids_named() {
    let (router, engine) = emulator();

    // An account that holds no association is answered the empty set, not a refusal: there
    // is nothing to report and nothing has gone wrong.
    let empty = describe_ok("an account with no association", &router, &[]).await;
    assert!(
        reported_ids(&empty).is_empty(),
        "an account with no association reports none, got {empty}"
    );
    assert_eq!(
        element(&empty, "nextToken"),
        None,
        "a single short page carries no continuation token, got {empty}"
    );

    let seeded: Vec<(&str, Seed, String)> = rows()
        .into_iter()
        .map(|(case, seed)| {
            let id = seed_association(&engine, &seed);
            (case, seed, id)
        })
        .collect();
    let every_id: BTreeSet<String> = seeded.iter().map(|(_, _, id)| id.clone()).collect();

    let all = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_id_set(&all),
        every_id,
        "the describe reports every stored association, got {all}"
    );

    for (case, seed, id) in &seeded {
        let body = describe_ok(
            case,
            &router,
            &indexed("IpamInternetRegistryAssociationId", &[id.as_str()]),
        )
        .await;
        assert_eq!(
            reported_ids(&body),
            vec![id.clone()],
            "case {case:?}: naming one id reports exactly that association, got {body}"
        );
        assert_eq!(
            element(&body, "ipamId").as_deref(),
            Some(seed.ipam_id),
            "case {case:?}: expected the IPAM the association belongs to, got {body}"
        );
        assert_eq!(
            element(&body, "ipamRegion").as_deref(),
            Some(seed.region),
            "case {case:?}: expected the Region of that IPAM, got {body}"
        );
        assert_eq!(
            element(&body, "rir").as_deref(),
            Some(seed.rir),
            "case {case:?}: expected the registry the association is with, got {body}"
        );
        assert_eq!(
            element(&body, "organizationHandle").as_deref(),
            Some(seed.organization_handle),
            "case {case:?}: expected the handle at that registry, got {body}"
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            seed.description,
            "case {case:?}: expected the stored description, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(seed.state),
            "case {case:?}: expected the state the association sits in, got {body}"
        );
        assert_eq!(
            element(&body, "stateMessage").as_deref(),
            seed.state_message,
            "case {case:?}: expected the stored state message, got {body}"
        );
        assert_eq!(
            element(&body, "ownerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the account that owns the association, got {body}"
        );
        let arn = element(&body, "ipamInternetRegistryAssociationArn").unwrap_or_default();
        assert!(
            arn.ends_with(&format!(
                "{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION}/{id}"
            )),
            "case {case:?}: expected the ARN to name this association, got {arn:?} in {body}"
        );
        assert_eq!(
            elements(&body, "key"),
            seed.tags
                .iter()
                .map(|(key, _)| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the tags assigned to the association, got {body}"
        );
    }

    // Two ids select both associations and nothing else.
    let (first, second) = (seeded[0].2.clone(), seeded[3].2.clone());
    let pair = describe_ok(
        "two ids",
        &router,
        &indexed(
            "IpamInternetRegistryAssociationId",
            &[first.as_str(), second.as_str()],
        ),
    )
    .await;
    assert_eq!(
        reported_id_set(&pair),
        [first, second].into_iter().collect::<BTreeSet<String>>(),
        "naming two ids reports exactly those associations, got {pair}"
    );

    // A report does not change what a later caller sees: the same request answers the same
    // associations (every response carries its own `requestId`, which is of the call rather
    // than of the resources), and the store still holds every association.
    let again = describe_ok("the whole store again", &router, &[]).await;
    assert_eq!(
        reported_ids(&again),
        reported_ids(&all),
        "reading the associations twice must report the same ones, got {again}"
    );
    for member in [
        "ipamId",
        "ipamRegion",
        "rir",
        "organizationHandle",
        "description",
        "state",
        "stateMessage",
        "ownerId",
    ] {
        assert_eq!(
            elements(&again, member),
            elements(&all, member),
            "reading the associations twice must report the same {member}, got {again}"
        );
    }
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .ipam_internet_registry_associations
            .len(),
        every_id.len(),
        "a report stores nothing and removes nothing"
    );
}

/// The rule: every filter the operation resolves selects on one member of the reported
/// resource, its values joined with OR and its filters with AND, and a filter name it cannot
/// resolve is refused rather than answered as "no results".
///
/// The rows span the member catalog: each id, the derived ARN, the account that owns it, the
/// IPAM and its Region, the registry enum, the handle, the description (exactly and with a
/// wildcard), the state, the state message, a member no association carries, and the three
/// tag filters — including entries the issue that prompted this suite never mentioned.
#[tokio::test]
async fn describing_selects_by_every_filter_it_resolves() {
    let (router, engine) = emulator();
    let seeded: Vec<(&str, Seed, String)> = rows()
        .into_iter()
        .map(|(case, seed)| {
            let id = seed_association(&engine, &seed);
            (case, seed, id)
        })
        .collect();
    let id_of = |index: usize| seeded[index].2.clone();
    let every_id: BTreeSet<String> = seeded.iter().map(|(_, _, id)| id.clone()).collect();

    let arin_id = id_of(0);
    let ripe_id = id_of(1);
    let apnic_id = id_of(2);
    let lacnic_id = id_of(3);
    let nicbr_id = id_of(4);

    let arn = {
        let body = describe_ok(
            "the arin association's ARN",
            &router,
            &indexed("IpamInternetRegistryAssociationId", &[arin_id.as_str()]),
        )
        .await;
        element(&body, "ipamInternetRegistryAssociationArn").expect("the association's ARN")
    };

    // (case, filter name, values, the associations that match)
    let cases: Vec<(&str, &str, Vec<String>, BTreeSet<String>)> = vec![
        (
            "by its own id",
            "ipam-internet-registry-association-id",
            vec![ripe_id.clone()],
            [ripe_id.clone()].into_iter().collect(),
        ),
        (
            "by two ids, joined with OR",
            "ipam-internet-registry-association-id",
            vec![ripe_id.clone(), nicbr_id.clone()],
            [ripe_id.clone(), nicbr_id.clone()].into_iter().collect(),
        ),
        (
            "by the derived ARN",
            "ipam-internet-registry-association-arn",
            vec![arn.clone()],
            [arin_id.clone()].into_iter().collect(),
        ),
        (
            "by the owning account, which every association shares",
            "owner-id",
            vec![engine::emulator_account_id().to_string()],
            every_id.clone(),
        ),
        (
            "by the IPAM only one association belongs to",
            "ipam-id",
            vec!["ipam-00000000000000abc".to_string()],
            [apnic_id.clone()].into_iter().collect(),
        ),
        (
            "by a Region two associations share",
            "ipam-region",
            vec!["sa-east-1".to_string()],
            [lacnic_id.clone(), nicbr_id.clone()].into_iter().collect(),
        ),
        (
            "by the registry enum",
            "rir",
            vec!["apnic".to_string()],
            [apnic_id.clone()].into_iter().collect(),
        ),
        (
            "by the organization handle",
            "organization-handle",
            vec!["ORG-EX1-RIPE".to_string()],
            [ripe_id.clone()].into_iter().collect(),
        ),
        (
            "by the description",
            "description",
            vec!["apac space".to_string()],
            [apnic_id.clone()].into_iter().collect(),
        ),
        (
            "by the description with a wildcard",
            "description",
            vec!["*space".to_string()],
            [apnic_id.clone(), nicbr_id.clone()].into_iter().collect(),
        ),
        (
            "by a state two associations are in",
            "state",
            vec![STATE_ENABLE_COMPLETE.to_string()],
            [arin_id.clone(), ripe_id.clone(), nicbr_id.clone()]
                .into_iter()
                .collect(),
        ),
        (
            "by a state no association is in",
            "state",
            vec![IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE.to_string()],
            BTreeSet::new(),
        ),
        (
            "by a state message with a wildcard",
            "state-message",
            vec!["the registry*".to_string()],
            [lacnic_id.clone()].into_iter().collect(),
        ),
        (
            // No association carries the child request XML the BPKI setup would return, so
            // the filter selects nothing whatever value it names.
            "by a member no association carries",
            "child-request-xml",
            vec!["*".to_string()],
            BTreeSet::new(),
        ),
        (
            "by a tag value under one key",
            "tag:Name",
            vec!["edge".to_string()],
            [ripe_id.clone()].into_iter().collect(),
        ),
        (
            "by a tag key two associations carry",
            "tag-key",
            vec!["Team".to_string()],
            [apnic_id.clone(), nicbr_id.clone()].into_iter().collect(),
        ),
        (
            "by a tag value under any key",
            "tag-value",
            vec!["network".to_string()],
            [apnic_id.clone(), nicbr_id.clone()].into_iter().collect(),
        ),
        (
            "by a tag value no association carries",
            "tag:Name",
            vec!["delta".to_string()],
            BTreeSet::new(),
        ),
    ];

    for (case, name, values, expected) in cases {
        let mut params = vec![param("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            params.push(param(&format!("Filter.1.Value.{}", index + 1), value));
        }
        let body = describe_ok(case, &router, &params).await;
        assert_eq!(
            reported_id_set(&body),
            expected,
            "case {case:?}: expected the filter to select {expected:?}, got {body}"
        );
    }

    // Two filters hold together with AND.
    let both = describe_ok(
        "two filters",
        &router,
        &[
            param("Filter.1.Name", "state"),
            param("Filter.1.Value.1", STATE_ENABLE_COMPLETE),
            param("Filter.2.Name", "tag-key"),
            param("Filter.2.Value.1", "Team"),
        ],
    )
    .await;
    assert_eq!(
        reported_ids(&both),
        vec![nicbr_id.clone()],
        "two filters are joined with AND, got {both}"
    );

    // An id narrows the same report as a filter, and the two hold together.
    let narrowed = describe_ok(
        "an id and a filter that disagree",
        &router,
        &{
            let mut params = indexed("IpamInternetRegistryAssociationId", &[arin_id.as_str()]);
            params.push(param("Filter.1.Name", "rir"));
            params.push(param("Filter.1.Value.1", "apnic"));
            params
        },
    )
    .await;
    assert!(
        reported_ids(&narrowed).is_empty(),
        "an id and a filter that select different associations report none, got {narrowed}"
    );

    // A filter name this operation does not resolve is refused.
    let (status, body) = describe(
        &router,
        &[
            param("Filter.1.Name", "association-colour"),
            param("Filter.1.Value.1", "blue"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "an unresolvable filter name is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an unresolvable filter name is refused as an invalid value, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "The filter 'association-colour' is invalid",
        "the refusal names the filter it could not resolve, got {body}"
    );
}

/// The rule: the describe answers at most `MaxResults` associations and a `NextToken` naming
/// where the next page starts, that token continues from the end of the page before it, and
/// the token is absent once there are no more results — while a request naming no page size
/// is not paged at all ("if not specified, all available results are returned",
/// `DescribeIpamInternetRegistryAssociationsRequest$MaxResults`). A token this service did not
/// mint is refused.
#[tokio::test]
async fn describing_pages_in_a_stable_order() {
    let (router, engine) = emulator();
    // The model's `IpamMaxResults` has a minimum of 5, so paging needs more than five
    // associations for a second page to exist at all.
    let mut created: BTreeSet<String> = BTreeSet::new();
    for nth in 0..7 {
        let seed = Seed {
            ipam_id: IPAM_ID,
            region: REGION,
            rir: RIR_VALUES[nth % RIR_VALUES.len()],
            organization_handle: "EXAMPLE-ORG",
            description: None,
            state: STATE_ENABLE_COMPLETE,
            state_message: None,
            tags: &[],
        };
        created.insert(seed_association(&engine, &seed));
    }

    for page_size in [5usize, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} associations, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: the paging walk must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages come in association id order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: paging reports every association exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no association is reported twice, got {seen:?}"
        );
    }

    // A request that names no page size is not paged at all: every association in one answer
    // and no token to follow.
    let unpaged = describe_ok("no page size asked for", &router, &[]).await;
    assert_eq!(
        reported_ids(&unpaged).into_iter().collect::<BTreeSet<String>>(),
        created,
        "a request naming no MaxResults reports every association, got {unpaged}"
    );
    assert_eq!(
        element(&unpaged, "nextToken"),
        None,
        "an unpaged answer carries no continuation token, got {unpaged}"
    );

    let (status, body) = describe(&router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "a token this service did not mint is refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "a bogus token is refused as an invalid pagination token, got {body}"
    );
    assert_eq!(
        error_message(&body),
        ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
        "the refusal carries EC2's own pagination-token sentence, got {body}"
    );
}

/// The rule: an id outside the form this service ISSUES is refused with this resource's
/// `.Malformed` before any lookup, an id of that form the account holds no association for
/// with its `.NotFound`, and `DryRun` answers the permission question instead of reporting.
#[tokio::test]
async fn describing_refuses_an_id_it_cannot_resolve() {
    let (router, engine) = emulator();
    let (_, seed) = rows().into_iter().next().expect("a seeded row");
    let held = seed_association(&engine, &seed);

    // (case, the id the request names, the code EC2 answers)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            // The id the recorded corpus probes: a seventeen-hex-digit suffix under a prefix
            // this API does not issue.
            "a suffix of the long form under a prefix this API does not issue",
            "ipam-ira-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "an id of another resource kind",
            "ipam-pool-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a suffix that is not hexadecimal",
            "ipam-ir-assoc-0000000000000000g",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "an uppercase suffix",
            "ipam-ir-assoc-0000000000000000F",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a suffix of neither published length",
            "ipam-ir-assoc-000000000",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "not an id at all",
            "my-association",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a well-formed long-form id the account holds no association for",
            "ipam-ir-assoc-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.NotFound",
        ),
        (
            "a well-formed short-form id the account holds no association for",
            "ipam-ir-assoc-0000000f",
            "InvalidIpamInternetRegistryAssociationId.NotFound",
        ),
    ];

    for (case, requested_id, expected_code) in rows {
        // The id is named beside one the account DOES hold, so a refusal is about the id the
        // request could not resolve rather than about the request carrying nothing known.
        let (status, body) = describe(
            &router,
            &indexed(
                "IpamInternetRegistryAssociationId",
                &[held.as_str(), requested_id],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got body {body}"
        );
        let expected_message = if expected_code.ends_with(".Malformed") {
            format!(
                "The {RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} ID \
                 '{requested_id}' is malformed"
            )
        } else {
            format!(
                "The {RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} ID \
                 '{requested_id}' does not exist"
            )
        };
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the refusal to name the id it could not resolve, got {body}"
        );
    }

    // "Checks whether you have the required permissions for the operation, without actually
    // making the request": every caller of this emulator is permitted.
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "a dry run answers the permission question, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a permitted caller's dry run is DryRunOperation, got {body}"
    );

    // The association the account holds is still reported, unchanged by any refusal above.
    let all = describe_ok("after the refusals", &router, &[]).await;
    assert_eq!(
        reported_ids(&all),
        vec![held],
        "a refused describe changes nothing the account holds, got {all}"
    );
}

/// The rule: a request is refused BEFORE any work proportional to what it carries when it
/// names more ids, more filters, more values per filter or a longer value than this service
/// will match — the same per-factor caps every other paginated describe in this crate
/// applies.
///
/// The rows are the four factors, each one past its published cap; the rows inside the cap
/// show the caps are not refusing requests they should serve.
#[tokio::test]
async fn describing_refuses_more_filtering_than_it_will_do() {
    let (router, engine) = emulator();
    let (_, seed) = rows().into_iter().next().expect("a seeded row");
    let held = seed_association(&engine, &seed);

    let many_filters = |count: usize| -> Vec<Param> {
        (0..count)
            .flat_map(|index| {
                vec![
                    param(&format!("Filter.{}.Name", index + 1), "owner-id"),
                    param(
                        &format!("Filter.{}.Value.1", index + 1),
                        engine::emulator_account_id(),
                    ),
                ]
            })
            .collect()
    };
    let many_values = |count: usize| -> Vec<Param> {
        let mut params = vec![param(
            "Filter.1.Name",
            "ipam-internet-registry-association-id",
        )];
        // Synthesised ids for every position but the last, which carries the id the store
        // really minted — so a list AT the cap exercises the cap and still selects the
        // association.
        params.extend((0..count.saturating_sub(1)).map(|index| {
            param(
                &format!("Filter.1.Value.{}", index + 1),
                &format!("ipam-ir-assoc-{:017x}", index + 1),
            )
        }));
        params.push(param(&format!("Filter.1.Value.{count}"), &held));
        params
    };
    let many_ids = |count: usize| -> Vec<Param> {
        (0..count)
            .map(|index| {
                param(
                    &format!("IpamInternetRegistryAssociationId.{}", index + 1),
                    &held,
                )
            })
            .collect()
    };

    // (case, the request, the code EC2 answers)
    let refused: Vec<RefusalCase> = vec![
        (
            "one filter past the cap",
            many_filters(ec2::state::MAX_FILTERS + 1),
            "InvalidParameterValue",
        ),
        (
            "one value past the cap",
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            "InvalidParameterValue",
        ),
        (
            "a filter value one byte past the cap",
            vec![
                param("Filter.1.Name", "description"),
                param(
                    "Filter.1.Value.1",
                    &"d".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
                ),
            ],
            "InvalidParameterValue",
        ),
        (
            "one id past the cap",
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in refused {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got body {body}"
        );
    }

    // At the cap, each factor is served and still selects the association the account holds.
    for (case, params) in [
        ("filters at the cap", many_filters(ec2::state::MAX_FILTERS)),
        (
            "values at the cap",
            many_values(ec2::state::MAX_FILTER_VALUES),
        ),
        ("ids at the cap", many_ids(ec2::state::MAX_RESOURCE_IDS)),
    ] {
        let body = describe_ok(case, &router, &params).await;
        assert_eq!(
            reported_ids(&body),
            vec![held.clone()],
            "case {case:?}: a request at the cap is served, got {body}"
        );
    }
}

/// The rule: the describe reports the state the store is ACTUALLY in, including the move the
/// clock had already made due — so an association a delete started on reads back
/// `delete-in-progress` straight after that call and `delete-complete` once the span has
/// passed, and reporting it is what a later caller sees too.
#[tokio::test]
async fn describing_reads_back_a_deletion_and_the_move_the_clock_owed() {
    let (router, engine) = emulator();
    let (_, seed) = rows().into_iter().next().expect("a seeded row");
    let association_id = seed_association(&engine, &seed);

    let (status, body) = call(
        &router,
        REGION,
        "DeleteIpamInternetRegistryAssociation",
        &[param("IpamInternetRegistryAssociationId", &association_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting the seeded association must succeed, got {status} with body {body}"
    );

    let deleting = describe_ok(
        "straight after the delete",
        &router,
        &indexed(
            "IpamInternetRegistryAssociationId",
            &[association_id.as_str()],
        ),
    )
    .await;
    assert_eq!(
        element(&deleting, "state").as_deref(),
        Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS),
        "a deletion under way reads back as still under way, got {deleting}"
    );
    assert_eq!(
        element(&deleting, "stateMessage").as_deref(),
        Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE),
        "the report carries the message of the state it is in, got {deleting}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let deleted = describe_ok(
        "once the move the clock owed is due",
        &router,
        &indexed(
            "IpamInternetRegistryAssociationId",
            &[association_id.as_str()],
        ),
    )
    .await;
    assert_eq!(
        element(&deleted, "state").as_deref(),
        Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE),
        "the move the clock owed is reported once it is due, got {deleted}"
    );
    assert_eq!(
        element(&deleted, "stateMessage").as_deref(),
        Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE),
        "the finished deletion carries its own message, got {deleted}"
    );

    // The state the describe reported is the state the store holds, so a filter on it selects
    // the association and the next caller sees the same thing.
    let by_state = describe_ok(
        "by the state the clock moved it to",
        &router,
        &[
            param("Filter.1.Name", "state"),
            param(
                "Filter.1.Value.1",
                IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
            ),
        ],
    )
    .await;
    assert_eq!(
        reported_ids(&by_state),
        vec![association_id],
        "the reported state is the stored one, got {by_state}"
    );
}
