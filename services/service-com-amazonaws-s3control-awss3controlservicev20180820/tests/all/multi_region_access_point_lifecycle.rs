// SPDX-License-Identifier: CC0-1.0

//! The S3 Control Multi-Region Access Point lifecycle, driven end to end over
//! HTTP.
//!
//! The rule under test: a `CreateMultiRegionAccessPoint` records ONE
//! Multi-Region Access Point per (account, name) — the buckets it associates,
//! the public-access block it was given and a service-assigned alias — and
//! answers the token of the asynchronous request that created it, so every later
//! `GetMultiRegionAccessPoint` of that name reports exactly what the create
//! submitted while `DescribeMultiRegionAccessPointOperation` of that token
//! reports that request. Both calls are asynchronous: the access point is
//! `CREATING` and the request `INPROGRESS` until the clock owes the outcome, and
//! `READY` / `SUCCEEDED` after it, with nobody having to call again. A create
//! under a name the account already holds is accepted like any other — the call
//! answers a token and binds no errors — and the conflict settles as THAT
//! request's outcome, leaving one record under the name; a byte-identical retry
//! is handed the first call's token rather than a second resource, and a
//! `DeleteMultiRegionAccessPoint` moves the record to `DELETING` and releases it
//! once the clock owes that.
//!
//! Every row below differs along something that rule generalises over: how many
//! buckets the access point associates and whether they name their owning
//! account, whether it carries a public-access block and which of its four
//! settings are on, the length of the name (both modelled bounds), the
//! idempotency token's characters, the Region the request is signed for, and the
//! account that owns it.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{leaves_of, Leaf};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ASYNC_REQUEST_FAILED, ASYNC_REQUEST_IN_PROGRESS, ASYNC_REQUEST_SUCCEEDED,
    MULTI_REGION_ACCESS_POINT_SETTLE_AFTER, STATUS_CREATING, STATUS_DELETING, STATUS_READY,
};

/// A second signing Region, used to pin that a request token's ARN names the
/// Region its request was signed for rather than a fixed one.
const OTHER_REGION: &str = "eu-central-1";

/// A request id in the shape this service mints one, that no request was ever
/// answered with: the nil-valued UUID, whose text is `8-4-4-4-12` hex like
/// every minted one.
const INVENTED_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The longest name the modelled `MultiRegionAccessPointName` admits: `len
/// 0..50` with pattern `^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`, so 50 characters.
const LONGEST_NAME: &str = "mrapnamemrapnamemrapnamemrapnamemrapnamemrapname50";

/// The shortest name that pattern admits: one leading character, one middle, one
/// trailing.
const SHORTEST_NAME: &str = "abc";

/// The tail the modelled `MultiRegionAccessPointAlias` pattern
/// (`^[a-z][a-z0-9]*[.]mrap$`) requires.
const ALIAS_TAIL: &str = ".mrap";

/// The modelled `MultiRegionAccessPointAlias` maximum length.
const ALIAS_MAX_LEN: usize = 63;

/// How long to wait before reading a transition the clock owes: longer than the
/// span the service holds a Multi-Region Access Point in its transient status,
/// so the settled outcome is due whether or not anybody called.
pub(crate) fn settled_wait() -> Duration {
    MULTI_REGION_ACCESS_POINT_SETTLE_AFTER + Duration::from_millis(250)
}

/// One `CreateMultiRegionAccessPoint` request: the members the model lets a
/// caller vary, plus the account and Region the call is made under.
///
/// `pub(crate)`, with [`create`] below it: the sibling suite that drives the
/// Multi-Region Access Point SUB-RESOURCES creates its access points the same
/// way rather than building a second request body.
pub(crate) struct CreateCase {
    pub(crate) case: &'static str,
    pub(crate) name: &'static str,
    /// The `ClientToken` idempotency token (`^\S+$`, at most 64 characters).
    pub(crate) client_token: &'static str,
    /// One (bucket, owning account) pair per associated Region, in request
    /// order. The model's `Region` names the bucket and, for a cross-account
    /// bucket, its owner — never the Region itself.
    pub(crate) regions: Vec<(&'static str, Option<&'static str>)>,
    /// `BlockPublicAcls`, `IgnorePublicAcls`, `BlockPublicPolicy`,
    /// `RestrictPublicBuckets` — in the order
    /// `PublicAccessBlockConfiguration` declares them.
    pub(crate) public_access_block: Option<[bool; 4]>,
    pub(crate) region: &'static str,
    pub(crate) account: &'static str,
}

impl CreateCase {
    pub(crate) fn new(
        case: &'static str,
        name: &'static str,
        client_token: &'static str,
        buckets: &[&'static str],
    ) -> Self {
        Self {
            case,
            name,
            client_token,
            regions: buckets.iter().map(|bucket| (*bucket, None)).collect(),
            public_access_block: None,
            region: REGION,
            account: ACCOUNT,
        }
    }

    pub(crate) fn request_body(&self) -> String {
        let mut xml = format!("<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">");
        xml.push_str(&format!(
            "<ClientToken>{}</ClientToken>",
            xml_escape(self.client_token)
        ));
        xml.push_str("<Details>");
        xml.push_str(&format!("<Name>{}</Name>", xml_escape(self.name)));
        if let Some([acls, ignore, policy, restrict]) = self.public_access_block {
            xml.push_str(&format!(
                "<PublicAccessBlock>\
                 <BlockPublicAcls>{acls}</BlockPublicAcls>\
                 <IgnorePublicAcls>{ignore}</IgnorePublicAcls>\
                 <BlockPublicPolicy>{policy}</BlockPublicPolicy>\
                 <RestrictPublicBuckets>{restrict}</RestrictPublicBuckets>\
                 </PublicAccessBlock>"
            ));
        }
        xml.push_str("<Regions>");
        for (bucket, bucket_account_id) in &self.regions {
            xml.push_str("<Region>");
            xml.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(bucket)));
            if let Some(bucket_account_id) = bucket_account_id {
                xml.push_str(&format!(
                    "<BucketAccountId>{}</BucketAccountId>",
                    xml_escape(bucket_account_id)
                ));
            }
            xml.push_str("</Region>");
        }
        xml.push_str("</Regions></Details></CreateMultiRegionAccessPointRequest>");
        xml
    }

    /// The regional entries a report of this access point must carry: the bucket
    /// as submitted, the Region the request was signed for, and the bucket's
    /// owning account when the request named one.
    fn expected_regions(&self) -> Vec<(String, String, String)> {
        self.regions
            .iter()
            .map(|(bucket, bucket_account_id)| {
                (
                    (*bucket).to_string(),
                    self.region.to_string(),
                    bucket_account_id.unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

/// The create cases every assertion below is run over. No row is merely a
/// renaming of another: each differs in something the rule generalises over.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase::new("one bucket, no optional members", "reports-mrap", "t-one", &["reports"]),
        CreateCase::new(
            "three buckets in request order",
            "wide-mrap",
            "t-three",
            &["reports", "reports-eu", "reports-ap"],
        ),
        CreateCase {
            regions: vec![("partner", Some(OTHER_ACCOUNT)), ("reports", None)],
            ..CreateCase::new("one cross-account bucket, one own", "mixed-mrap", "t-mixed", &[])
        },
        CreateCase {
            public_access_block: Some([true, true, true, true]),
            ..CreateCase::new("public access fully blocked", "locked-mrap", "t-locked", &["finance"])
        },
        CreateCase {
            public_access_block: Some([true, false, false, true]),
            ..CreateCase::new(
                "public access partly blocked",
                "half-locked-mrap",
                "t-half",
                &["finance"],
            )
        },
        CreateCase::new("shortest name the model admits", SHORTEST_NAME, "t-short", &["reports"]),
        CreateCase::new("longest name the model admits", LONGEST_NAME, "t-long", &["reports"]),
        CreateCase::new(
            "idempotency token of punctuation and digits",
            "token-shaped-mrap",
            "a+b/c=9_%",
            &["reports"],
        ),
        CreateCase {
            region: OTHER_REGION,
            ..CreateCase::new("signed for another Region", "eu-mrap", "t-eu", &["reports-frankfurt"])
        },
    ]
}

pub(crate) async fn create(emulator: &Emulator, case: &CreateCase) -> Reply {
    // The prerequisite of every create below: a Multi-Region Access Point IS
    // its set of associated buckets, so S3 Control carries a create out only
    // over buckets that exist — each recorded in the Region this request's own
    // report then names it in.
    for (bucket, _) in &case.regions {
        emulator.provision_bucket_in(case.case, bucket, case.region);
    }
    call_in_region(
        &emulator.router,
        case.region,
        "POST",
        "/v20180820/async-requests/mrap/create",
        case.account,
        &case.request_body(),
    )
    .await
}

async fn get(emulator: &Emulator, region: &str, account: &str, name: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/mrap/instances/{name}"),
        account,
        "",
    )
    .await
}

pub(crate) async fn describe(
    emulator: &Emulator,
    region: &str,
    account: &str,
    token: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        &format!("/v20180820/async-requests/mrap/{token}"),
        account,
        "",
    )
    .await
}

/// `ListMultiRegionAccessPoints` for one account.
async fn list(emulator: &Emulator, region: &str, account: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/mrap/instances",
        account,
        "",
    )
    .await
}

async fn delete(
    emulator: &Emulator,
    region: &str,
    account: &str,
    name: &str,
    client_token: &str,
) -> Reply {
    let body = format!(
        "<DeleteMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name></Details>\
         </DeleteMultiRegionAccessPointRequest>",
        xml_escape(client_token),
        xml_escape(name)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/async-requests/mrap/delete",
        account,
        &body,
    )
    .await
}

/// The text of the one leaf at `path`, or `None` when the answer carries none.
pub(crate) fn leaf(leaves: &[Leaf], path: &str) -> Option<String> {
    leaves
        .iter()
        .find(|(leaf_path, _)| leaf_path == path)
        .map(|(_, value)| value.clone())
}

/// The regional entries under `prefix`, as (bucket, Region, bucket account id)
/// in document order.
///
/// A `RegionReport` is written as `<Region><Bucket/><Region/><BucketAccountId/>`
/// — the element `Region` nested inside a `Region` — so the entries are read
/// from the leaf PATHS rather than by scanning for an element name: a new entry
/// starts at each `Bucket`, the first member the model declares.
fn region_entries(leaves: &[Leaf], prefix: &str) -> Vec<(String, String, String)> {
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for (path, value) in leaves {
        let Some(member) = path.strip_prefix(prefix) else {
            continue;
        };
        match member {
            "Bucket" => entries.push((value.clone(), String::new(), String::new())),
            "Region" => {
                if let Some(entry) = entries.last_mut() {
                    entry.1 = value.clone();
                }
            }
            "BucketAccountId" => {
                if let Some(entry) = entries.last_mut() {
                    entry.2 = value.clone();
                }
            }
            _ => panic!("unexpected member {member:?} of a regional entry"),
        }
    }
    entries
}

/// The prefix the leaf paths of a `GetMultiRegionAccessPoint` report's regional
/// entries carry.
const REPORT_REGION_PREFIX: &str = "AccessPoint/Regions/Region/";

/// Assert that `alias` is a service-assigned Multi-Region Access Point alias:
/// the modelled `MultiRegionAccessPointAlias` pattern `^[a-z][a-z0-9]*[.]mrap$`,
/// within the modelled 63-character maximum.
fn assert_alias_shape(case: &str, alias: &str) {
    let body = alias.strip_suffix(ALIAS_TAIL).unwrap_or_else(|| {
        panic!("case {case:?}: expected an alias ending in {ALIAS_TAIL:?}, got {alias:?}")
    });
    assert!(
        alias.len() <= ALIAS_MAX_LEN,
        "case {case:?}: expected an alias within {ALIAS_MAX_LEN} characters, got {alias:?}"
    );
    let mut characters = body.chars();
    let first = characters.next();
    assert!(
        first.is_some_and(|first| first.is_ascii_lowercase()),
        "case {case:?}: expected an alias starting with a lowercase letter, got {alias:?}"
    );
    assert!(
        characters.all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "case {case:?}: expected an alias of lowercase letters and digits, got {alias:?}"
    );
}

/// Assert that `token` is a request token ARN for `verb` in `region`: the
/// `async-request/mrap/<verb>/<id>` resource under the emulator's canonical
/// account, with a UUID id.
pub(crate) fn assert_token_shape(case: &str, token: &str, region: &str, verb: &str) {
    let prefix = format!(
        "arn:aws:s3:{region}:{}:async-request/mrap/{verb}/",
        ::emulator_server::emulator_account_id()
    );
    let id = token.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a request token starting {prefix:?}, got {token:?}")
    });
    assert_uuid_shape(case, id, token);
}

/// The lengths of the five hyphen-separated groups of a UUID's text, in order.
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// Assert that `id` is the text of a UUID: `8-4-4-4-12` lowercase hex, the
/// shape every service-assigned identifier of this service carries.
pub(crate) fn assert_uuid_shape(case: &str, id: &str, within: &str) {
    let groups: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(
        groups,
        UUID_GROUP_LENGTHS.to_vec(),
        "case {case:?}: expected a UUID request id, got {id:?} in {within:?}"
    );
    assert!(
        id.chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit()),
        "case {case:?}: expected hex digits in every group, got {id:?} in {within:?}"
    );
    assert!(
        id.chars().all(|character| !character.is_ascii_uppercase()),
        "case {case:?}: expected lowercase hex, got {id:?} in {within:?}"
    );
}

#[tokio::test]
async fn create_then_get_reports_the_stored_access_point_for_every_shape_of_create() {
    assert_eq!(
        LONGEST_NAME.len(),
        50,
        "the longest-name row must sit on the modelled maximum, not near it"
    );
    let emulator = Emulator::start();
    let mut aliases = BTreeSet::new();
    let mut tokens = BTreeSet::new();

    for case in create_cases() {
        let created = create(&emulator, &case).await;
        let read = get(&emulator, case.region, case.account, case.name).await;

        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateMultiRegionAccessPoint to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );
        let token = created.text("RequestTokenARN").unwrap_or_default();
        assert_token_shape(case.case, &token, case.region, "create");
        tokens.insert(token.clone());

        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetMultiRegionAccessPoint to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        let leaves = leaves_of(&read.body);
        assert_eq!(
            leaf(&leaves, "AccessPoint/Name"),
            Some(case.name.to_string()),
            "case {:?}: expected the report to name the created access point, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            leaf(&leaves, "AccessPoint/Status"),
            Some(STATUS_CREATING.to_string()),
            "case {:?}: expected the transient create status, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            region_entries(&leaves, REPORT_REGION_PREFIX),
            case.expected_regions(),
            "case {:?}: expected the associated buckets as submitted, got body {}",
            case.case,
            read.body
        );
        // Public access is blocked by default, so a create that supplied no
        // configuration leaves all four settings on.
        let block = case
            .public_access_block
            .unwrap_or([true, true, true, true])
            .map(|setting| setting.to_string())
            .to_vec();
        let reported_block = [
            "BlockPublicAcls",
            "IgnorePublicAcls",
            "BlockPublicPolicy",
            "RestrictPublicBuckets",
        ]
        .iter()
        .map(|member| leaf(&leaves, &format!("AccessPoint/PublicAccessBlock/{member}")))
        .collect::<Option<Vec<String>>>();
        assert_eq!(
            reported_block,
            Some(block),
            "case {:?}: expected the effective public-access block, got body {}",
            case.case, read.body
        );
        assert!(
            leaf(&leaves, "AccessPoint/CreatedAt").is_some_and(|created_at| !created_at.is_empty()),
            "case {:?}: expected the report to carry CreatedAt, got body {}",
            case.case,
            read.body
        );

        let alias = leaf(&leaves, "AccessPoint/Alias").unwrap_or_default();
        assert_alias_shape(case.case, &alias);
        assert!(
            aliases.insert(alias.clone()),
            "case {:?}: expected an alias no other access point carries, got {alias:?}",
            case.case
        );

        // Reading twice changes nothing a later caller sees: an alias is one
        // value per access point, not one per read.
        let again = get(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            leaf(&leaves_of(&again.body), "AccessPoint/Alias"),
            Some(alias.clone()),
            "case {:?}: expected the alias to be stable across reads, got body {}",
            case.case,
            again.body
        );
    }

    assert_eq!(
        tokens.len(),
        create_cases().len(),
        "expected one request token per request, got {tokens:?}"
    );
}

#[tokio::test]
async fn a_created_access_point_and_its_request_settle_once_the_clock_owes_them() {
    let emulator = Emulator::start();
    let cases = create_cases();
    let mut tokens = Vec::new();

    for case in &cases {
        let created = create(&emulator, case).await;
        let token = created.text("RequestTokenARN").unwrap_or_default();
        let described = describe(&emulator, case.region, case.account, &token).await;
        let leaves = leaves_of(&described.body);
        assert_eq!(
            described.status, 200,
            "case {:?}: expected DescribeMultiRegionAccessPointOperation to answer 200, \
             got {} with body {}",
            case.case, described.status, described.body
        );
        assert_eq!(
            leaf(&leaves, "AsyncOperation/RequestStatus"),
            Some(ASYNC_REQUEST_IN_PROGRESS.to_string()),
            "case {:?}: expected the request to be in progress before the clock owes it, \
             got body {}",
            case.case,
            described.body
        );
        tokens.push(token);
    }

    // The clock, not another call, is what completes the requests.
    std::thread::sleep(settled_wait());

    for (case, token) in cases.iter().zip(&tokens) {
        let read = get(&emulator, case.region, case.account, case.name).await;
        let leaves = leaves_of(&read.body);
        assert_eq!(
            leaf(&leaves, "AccessPoint/Status"),
            Some(STATUS_READY.to_string()),
            "case {:?}: expected the settled create status, got body {}",
            case.case,
            read.body
        );
        // The settled status is terminal: a second read does not move it on.
        let again = get(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            leaf(&leaves_of(&again.body), "AccessPoint/Status"),
            Some(STATUS_READY.to_string()),
            "case {:?}: expected the settled status to stay settled, got body {}",
            case.case,
            again.body
        );

        let described = describe(&emulator, case.region, case.account, token).await;
        let described_leaves = leaves_of(&described.body);
        assert_eq!(
            leaf(&described_leaves, "AsyncOperation/RequestStatus"),
            Some(ASYNC_REQUEST_SUCCEEDED.to_string()),
            "case {:?}: expected the request to have succeeded, got body {}",
            case.case,
            described.body
        );
        let regional: Vec<Option<String>> = described_leaves
            .iter()
            .filter(|(path, _)| {
                path == "AsyncOperation/ResponseDetails/MultiRegionAccessPointDetails/\
                         Regions/Region/RequestStatus"
            })
            .map(|(_, value)| Some(value.clone()))
            .collect();
        assert_eq!(
            regional,
            vec![Some(ASYNC_REQUEST_SUCCEEDED.to_string()); case.regions.len()],
            "case {:?}: expected one succeeded entry per associated Region, got body {}",
            case.case,
            described.body
        );
    }
}

#[tokio::test]
async fn describe_reports_the_parameters_the_request_submitted() {
    let emulator = Emulator::start();

    for case in create_cases() {
        let created = create(&emulator, &case).await;
        let token = created.text("RequestTokenARN").unwrap_or_default();
        let described = describe(&emulator, case.region, case.account, &token).await;
        let leaves = leaves_of(&described.body);

        assert_eq!(
            leaf(&leaves, "AsyncOperation/Operation"),
            Some("CreateMultiRegionAccessPoint".to_string()),
            "case {:?}: expected the request to name the operation that made it, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            leaf(&leaves, "AsyncOperation/RequestTokenARN"),
            Some(token.clone()),
            "case {:?}: expected the request to carry the token it was read by, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            leaf(
                &leaves,
                "AsyncOperation/RequestParameters/CreateMultiRegionAccessPointRequest/Name"
            ),
            Some(case.name.to_string()),
            "case {:?}: expected the create parameters to name the access point, got body {}",
            case.case,
            described.body
        );
        let submitted_buckets: Vec<String> = leaves
            .iter()
            .filter(|(path, _)| {
                path == "AsyncOperation/RequestParameters/\
                         CreateMultiRegionAccessPointRequest/Regions/Region/Bucket"
            })
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(
            submitted_buckets,
            case.regions
                .iter()
                .map(|(bucket, _)| (*bucket).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the create parameters to carry the buckets submitted, \
             got body {}",
            case.case,
            described.body
        );
        // The parameters carry the public-access block in force for the access
        // point the request asked for: the settings it supplied, or all four
        // blocked when it supplied none — the same settings the access point's
        // own report carries, so the two cannot disagree.
        let submitted_block = [
            "BlockPublicAcls",
            "IgnorePublicAcls",
            "BlockPublicPolicy",
            "RestrictPublicBuckets",
        ]
        .iter()
        .map(|member| {
            leaf(
                &leaves,
                &format!(
                    "AsyncOperation/RequestParameters/\
                     CreateMultiRegionAccessPointRequest/PublicAccessBlock/{member}"
                ),
            )
        })
        .collect::<Option<Vec<String>>>();
        assert_eq!(
            submitted_block,
            Some(
                case.public_access_block
                    .unwrap_or([true, true, true, true])
                    .map(|setting| setting.to_string())
                    .to_vec()
            ),
            "case {:?}: expected the create parameters to carry the effective public-access \
             block, got body {}",
            case.case,
            described.body
        );
        assert!(
            leaf(&leaves, "AsyncOperation/CreationTime")
                .is_some_and(|creation_time| !creation_time.is_empty()),
            "case {:?}: expected the request to carry its creation time, got body {}",
            case.case,
            described.body
        );
    }
}

/// RULE: `CreateMultiRegionAccessPoint` answers nothing but a request token and
/// binds no errors at all, so a create under a name the account already holds is
/// ACCEPTED — with a token of its own — and the conflict is the OUTCOME of that
/// request: it settles `FAILED` carrying
/// `MultiRegionAccessPointAlreadyOwnedByYou` in its `ErrorDetails`, exactly one
/// access point stands under the name, the one that was there is untouched, and
/// the failed request reports no per-Region result at all.
#[tokio::test]
async fn a_create_under_a_name_the_account_holds_fails_the_request_it_minted() {
    let emulator = Emulator::start();

    for case in create_cases() {
        let first = create(&emulator, &case).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected the first create to answer 200, got {} with body {}",
            case.case, first.status, first.body
        );
        let first_token = first.text("RequestTokenARN").unwrap_or_default();

        // A different idempotency token makes this a different request for a
        // name the account already holds — the loser of a create race.
        let second = CreateCase {
            client_token: "a-second-request",
            ..case
        };
        let losing = create(&emulator, &second).await;
        assert_eq!(
            losing.status, 200,
            "case {:?}: expected the losing create to be accepted with 200, got {} with body {}",
            second.case, losing.status, losing.body
        );
        let losing_token = losing.text("RequestTokenARN").unwrap_or_default();
        assert_ne!(
            losing_token, first_token,
            "case {:?}: expected the two creates to be handed distinct request tokens, got \
             body {}",
            second.case, losing.body
        );

        // The clock, not another call, is what settles both requests.
        std::thread::sleep(settled_wait());

        let winner = describe(&emulator, second.region, second.account, &first_token).await;
        assert_eq!(
            leaf(&leaves_of(&winner.body), "AsyncOperation/RequestStatus"),
            Some(ASYNC_REQUEST_SUCCEEDED.to_string()),
            "case {:?}: expected exactly one request to have succeeded, got body {}",
            second.case,
            winner.body
        );

        let loser = describe(&emulator, second.region, second.account, &losing_token).await;
        let loser_leaves = leaves_of(&loser.body);
        assert_eq!(
            leaf(&loser_leaves, "AsyncOperation/RequestStatus"),
            Some(ASYNC_REQUEST_FAILED.to_string()),
            "case {:?}: expected the losing request to have failed, got body {}",
            second.case,
            loser.body
        );
        assert_eq!(
            leaf(
                &loser_leaves,
                "AsyncOperation/ResponseDetails/ErrorDetails/Code"
            ),
            Some(NAME_CONFLICT_CODE.to_string()),
            "case {:?}: expected the name-conflict code in the failure's error details, got \
             body {}",
            second.case,
            loser.body
        );
        assert_eq!(
            leaf(
                &loser_leaves,
                "AsyncOperation/ResponseDetails/ErrorDetails/Message"
            ),
            Some(format!(
                "A Multi-Region Access Point with the same name already exists: {}",
                second.name
            )),
            "case {:?}: expected the failure to name the contested access point, got body {}",
            second.case,
            loser.body
        );
        let regional: Vec<String> = loser_leaves
            .iter()
            .filter(|(path, _)| {
                path == "AsyncOperation/ResponseDetails/MultiRegionAccessPointDetails/\
                         Regions/Region/RequestStatus"
            })
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(
            regional,
            Vec::<String>::new(),
            "case {:?}: expected a failed request to report no per-Region result, got body {}",
            second.case,
            loser.body
        );

        // The losing request left the first access point exactly as it was, and
        // the name still stands for one record.
        let read = get(&emulator, second.region, second.account, second.name).await;
        let leaves = leaves_of(&read.body);
        assert_eq!(
            region_entries(&leaves, REPORT_REGION_PREFIX),
            second.expected_regions(),
            "case {:?}: expected the losing create to have changed nothing, got body {}",
            second.case,
            read.body
        );
        assert_eq!(
            leaf(&leaves, "AccessPoint/Status"),
            Some(STATUS_READY.to_string()),
            "case {:?}: expected the access point that won to be settled, got body {}",
            second.case,
            read.body
        );
        let listed = list(&emulator, second.region, second.account).await;
        let named = listed
            .body
            .matches(&format!("<Name>{}</Name>", second.name))
            .count();
        assert_eq!(
            named, 1,
            "case {:?}: expected exactly one access point under the contested name, got body {}",
            second.case, listed.body
        );
    }
}

/// The `ErrorDetails$Code` the loser of a create race settles with.
const NAME_CONFLICT_CODE: &str = "MultiRegionAccessPointAlreadyOwnedByYou";

#[tokio::test]
async fn a_byte_identical_retry_is_handed_the_first_token_and_creates_nothing() {
    let emulator = Emulator::start();

    for case in create_cases() {
        let first = create(&emulator, &case).await;
        let retried = create(&emulator, &case).await;
        assert_eq!(
            retried.status, 200,
            "case {:?}: expected an identical retry to answer 200, got {} with body {}",
            case.case, retried.status, retried.body
        );
        assert_eq!(
            retried.text("RequestTokenARN"),
            first.text("RequestTokenARN"),
            "case {:?}: expected the retry to be handed the first request's token, got body {}",
            case.case,
            retried.body
        );

        // One record, not two: the access point still reports the one set of
        // buckets the request submitted.
        let read = get(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            region_entries(&leaves_of(&read.body), REPORT_REGION_PREFIX),
            case.expected_regions(),
            "case {:?}: expected the retry to have created nothing new, got body {}",
            case.case,
            read.body
        );
    }
}

#[tokio::test]
async fn a_delete_releases_the_access_point_once_the_clock_owes_it() {
    let emulator = Emulator::start();
    let cases = create_cases();

    for case in &cases {
        create(&emulator, case).await;
    }

    let mut tokens = Vec::new();
    for case in &cases {
        let deleted = delete(
            &emulator,
            case.region,
            case.account,
            case.name,
            "delete-once",
        )
        .await;
        let read = get(&emulator, case.region, case.account, case.name).await;

        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected DeleteMultiRegionAccessPoint to answer 200, got {} with body {}",
            case.case, deleted.status, deleted.body
        );
        let token = deleted.text("RequestTokenARN").unwrap_or_default();
        assert_token_shape(case.case, &token, case.region, "delete");
        assert_eq!(
            leaf(&leaves_of(&read.body), "AccessPoint/Status"),
            Some(STATUS_DELETING.to_string()),
            "case {:?}: expected the transient delete status, got body {}",
            case.case,
            read.body
        );
        tokens.push(token);
    }

    std::thread::sleep(settled_wait());

    for (case, token) in cases.iter().zip(&tokens) {
        let read = get(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: expected the released access point to be gone, got {} with body {}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.code(),
            "NoSuchMultiRegionAccessPoint",
            "case {:?}: expected the missing-access-point code, got body {}",
            case.case,
            read.body
        );

        let described = describe(&emulator, case.region, case.account, token).await;
        let leaves = leaves_of(&described.body);
        assert_eq!(
            leaf(&leaves, "AsyncOperation/Operation"),
            Some("DeleteMultiRegionAccessPoint".to_string()),
            "case {:?}: expected the delete request to name its operation, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            leaf(&leaves, "AsyncOperation/RequestStatus"),
            Some(ASYNC_REQUEST_SUCCEEDED.to_string()),
            "case {:?}: expected the delete request to have succeeded, got body {}",
            case.case,
            described.body
        );
        assert_eq!(
            leaf(
                &leaves,
                "AsyncOperation/RequestParameters/DeleteMultiRegionAccessPointRequest/Name"
            ),
            Some(case.name.to_string()),
            "case {:?}: expected the delete parameters to name the access point, got body {}",
            case.case,
            described.body
        );

        // The name is free again once the record is released: a fresh request
        // (a new idempotency token) creates a new access point under it.
        let recreated = create(
            &emulator,
            &CreateCase {
                case: case.case,
                name: case.name,
                client_token: "after-the-delete",
                regions: vec![("reports", None)],
                public_access_block: None,
                region: case.region,
                account: case.account,
            },
        )
        .await;
        assert_eq!(
            recreated.status, 200,
            "case {:?}: expected the released name to be creatable again, got {} with body {}",
            case.case, recreated.status, recreated.body
        );
        let read = get(&emulator, case.region, case.account, case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the recreated access point to be readable, got {} with body {}",
            case.case, read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_request_addressed_in_another_account_is_refused_before_the_access_point_is_looked_up() {
    let emulator = Emulator::start();
    let case = CreateCase::new("owned by one account", "private-mrap", "t-private", &["reports"]);
    create(&emulator, &case).await;

    // One row per call that names the account: a name the caller's own account
    // holds, a name nothing holds, and a create of a new name — all the same
    // refusal, so the answer says nothing about what the other account holds.
    for (label, reply) in [
        (
            "GetMultiRegionAccessPoint, name present",
            get(&emulator, REGION, OTHER_ACCOUNT, case.name).await,
        ),
        (
            "GetMultiRegionAccessPoint, name absent",
            get(&emulator, REGION, OTHER_ACCOUNT, "absent-mrap").await,
        ),
        (
            "DeleteMultiRegionAccessPoint",
            delete(&emulator, REGION, OTHER_ACCOUNT, case.name, "t-steal").await,
        ),
        (
            "CreateMultiRegionAccessPoint",
            create(
                &emulator,
                &CreateCase {
                    account: OTHER_ACCOUNT,
                    ..CreateCase::new("another account", "theirs-mrap", "t-other", &["reports"])
                },
            )
            .await,
        ),
    ] {
        assert_foreign_account_refused(label, OTHER_ACCOUNT, &reply);
    }

    // The owner still holds it.
    let owner = get(&emulator, REGION, ACCOUNT, case.name).await;
    assert_eq!(
        owner.status, 200,
        "expected the owner's read to still answer 200, got {} with body {}",
        owner.status, owner.body
    );
}

#[tokio::test]
async fn requests_and_names_the_store_does_not_hold_are_refused() {
    let emulator = Emulator::start();

    // A token no request was ever answered with, in the form the model admits.
    let invented = format!(
        "arn:aws:s3:{REGION}:{}:async-request/mrap/create/{INVENTED_REQUEST_ID}",
        ::emulator_server::emulator_account_id(),
    );
    let described = describe(&emulator, REGION, ACCOUNT, &invented).await;
    assert_eq!(
        described.status, 404,
        "expected an unknown request token to be refused with 404, got {} with body {}",
        described.status, described.body
    );
    assert_eq!(
        described.code(),
        "NoSuchAsyncRequest",
        "expected the not-found code for an unknown request token, got body {}",
        described.body
    );

    let read = get(&emulator, REGION, ACCOUNT, "never-created-mrap").await;
    assert_eq!(
        read.status, 404,
        "expected an unknown name to be refused with 404, got {} with body {}",
        read.status, read.body
    );

    let removed = delete(&emulator, REGION, ACCOUNT, "never-created-mrap", "t-ghost").await;
    assert_eq!(
        removed.status, 404,
        "expected deleting an unknown name to be refused with 404, got {} with body {}",
        removed.status, removed.body
    );
    assert_eq!(
        removed.code(),
        "NoSuchMultiRegionAccessPoint",
        "expected the missing-access-point code, got body {}",
        removed.body
    );
}

#[tokio::test]
async fn a_create_the_model_or_the_service_refuses_persists_nothing() {
    let emulator = Emulator::start();

    // One past the modelled 50-character maximum for a name: the boundary
    // validator refuses it before the handler runs.
    let too_long = format!("{LONGEST_NAME}x");
    let past_bound = call_in_region(
        &emulator.router,
        REGION,
        "POST",
        "/v20180820/async-requests/mrap/create",
        ACCOUNT,
        &format!(
            "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
             <ClientToken>t-past-bound</ClientToken><Details><Name>{too_long}</Name>\
             <Regions><Region><Bucket>reports</Bucket></Region></Regions></Details>\
             </CreateMultiRegionAccessPointRequest>"
        ),
    )
    .await;
    assert_eq!(
        past_bound.status, 400,
        "expected a name past the modelled maximum to be refused with 400, got {} with body {}",
        past_bound.status, past_bound.body
    );

    // A request associating no bucket at all: a Multi-Region Access Point IS its
    // set of associated buckets, so there is nothing for this one to be.
    let no_regions = call_in_region(
        &emulator.router,
        REGION,
        "POST",
        "/v20180820/async-requests/mrap/create",
        ACCOUNT,
        &format!(
            "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
             <ClientToken>t-no-regions</ClientToken><Details><Name>empty-mrap</Name>\
             <Regions></Regions></Details></CreateMultiRegionAccessPointRequest>"
        ),
    )
    .await;
    assert_eq!(
        no_regions.status, 400,
        "expected a create naming no bucket to be refused with 400, got {} with body {}",
        no_regions.status, no_regions.body
    );
    assert_eq!(
        (no_regions.code(), no_regions.text("Message")),
        (
            "InvalidRequest".to_string(),
            Some("List of regions cannot be empty".to_string())
        ),
        "expected the empty-region-list refusal in this service's own words, got body {}",
        no_regions.body
    );
    let read = get(&emulator, REGION, ACCOUNT, "empty-mrap").await;
    assert_eq!(
        read.status, 404,
        "expected the refused create to have persisted nothing, got {} with body {}",
        read.status, read.body
    );

    // The same request with one bucket is accepted, which is what makes the two
    // refusals above about the name and the bucket set rather than the route.
    let accepted = create(
        &emulator,
        &CreateCase::new("one bucket", "empty-mrap", "t-one-bucket", &["reports"]),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "expected the control row to be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
}
