// SPDX-License-Identifier: CC0-1.0

//! What a `CreateMultiRegionAccessPoint` DOES with the request it was given,
//! driven end to end over HTTP.
//!
//! Two rules are under test.
//!
//! The first is about the NAME: a Multi-Region Access Point name outside the
//! modelled `MultiRegionAccessPointName` (`len 0..50`, pattern
//! `^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`) is a request S3 Control will not carry
//! out, so it is refused with that service's own code and its one fixed
//! sentence about the name — never with the framework's validation vocabulary,
//! and never restating the value back to the caller — on every route that
//! carries such a name, whether in its request document or in its path.
//!
//! The second is about the BUCKETS: a Multi-Region Access Point IS its set of
//! associated buckets, so a create whose `Regions` list names one bucket more
//! than once, or names a bucket the object store does not hold, cannot be
//! carried out. The call binds no errors at all, so each is accepted with a
//! request token and reported as THAT request's outcome — `FAILED`, with
//! `ResponseDetails.ErrorDetails` naming the offending bucket and no per-Region
//! result — while the access point itself is never published, so every later
//! read of the name is the same 404 as a name nothing was ever created under.
//! A create over distinct, held buckets is the control row: it succeeds and the
//! access point is readable.
//!
//! Every row differs in something those rules generalise over: which character
//! rule the name breaks and which route carries it; how many buckets the create
//! associates, where in the list the repeated or absent one sits, how many
//! Regions they live in, whether the request supplied a public-access block,
//! and the Region and account the call is made under.
//!
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::time::{Duration, Instant};

use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, REGION, XMLNS,
};
use crate::multi_region_access_point_lifecycle::{
    assert_token_shape, describe, leaf, CreateCase,
};
use crate::storage_lens_configuration::leaves_of;

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ASYNC_REQUEST_FAILED, ASYNC_REQUEST_IN_PROGRESS, ASYNC_REQUEST_SUCCEEDED,
    DUPLICATE_ASSOCIATED_BUCKET_MESSAGE, INVALID_MULTI_REGION_ACCESS_POINT_NAME_MESSAGE,
    NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE, REQUEST_INVALID_CODE, STATUS_READY,
};

/// A second signing Region, so no row's outcome can depend on the one the
/// sibling suites sign for.
const OTHER_REGION: &str = "eu-central-1";

/// The code every absent-Multi-Region-Access-Point refusal carries, and its
/// fixed message.
const NO_SUCH_MRAP_CODE: &str = "NoSuchMultiRegionAccessPoint";
const NO_SUCH_MRAP_MESSAGE: &str = "The specified Multi-Region Access Point does not exist.";

/// How long a poll waits for the clock to owe an asynchronous request its
/// outcome: far longer than the span the service holds one in progress, so what
/// settles a request is the clock and never how fast this host happens to be.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between looks.
const SETTLE_INTERVAL: Duration = Duration::from_millis(20);

/// The four members of `PublicAccessBlockConfiguration`, in the order the model
/// declares them.
const PUBLIC_ACCESS_BLOCK_MEMBERS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// One `CreateMultiRegionAccessPoint` and the outcome its request must settle
/// with.
struct OutcomeRow {
    case: &'static str,
    name: &'static str,
    client_token: &'static str,
    /// The buckets the object store is to hold, each with the Region it lives
    /// in. A bucket the create names but this list does not is one the store
    /// holds nothing for.
    held: &'static [(&'static str, &'static str)],
    /// The buckets the create's `Regions` names, in request order — repeats
    /// included, which is the point of several rows.
    associated: &'static [&'static str],
    /// The `PublicAccessBlock` the request supplies, or `None` for a request
    /// that supplies none.
    public_access_block: Option<[bool; 4]>,
    region: &'static str,
    account: &'static str,
    /// The `ErrorDetails` (code, message) the request must settle with, or
    /// `None` for a create that is carried out. The `Resource` of a failure is
    /// always the Multi-Region Access Point the request was about, so it is
    /// read off [`Self::name`] rather than restated per row.
    failure: Option<(&'static str, &'static str)>,
}

impl OutcomeRow {
    /// The create request this row submits.
    fn create_case(&self) -> CreateCase {
        CreateCase {
            case: self.case,
            name: self.name,
            client_token: self.client_token,
            regions: self
                .associated
                .iter()
                .map(|bucket| (*bucket, None))
                .collect(),
            public_access_block: self.public_access_block,
            region: self.region,
            account: self.account,
        }
    }

    /// The public-access block settings in force for the access point this row
    /// asks for: the ones it supplied, or all four blocked when it supplied
    /// none (`CreateMultiRegionAccessPointInput$PublicAccessBlock`).
    fn effective_public_access_block(&self) -> Vec<String> {
        self.public_access_block
            .unwrap_or([true, true, true, true])
            .map(|setting| setting.to_string())
            .to_vec()
    }
}

/// The rows every assertion below is run over. No row is merely a rename of
/// another: each differs in how many buckets the create associates, where the
/// repeated or absent bucket sits in the list, how many Regions the buckets
/// live in, whether a public-access block was supplied, or the Region and
/// account the call is made under.
fn outcome_rows() -> Vec<OutcomeRow> {
    vec![
        OutcomeRow {
            case: "distinct buckets the store holds, in two Regions",
            name: "outcome-ok-mrap",
            client_token: "t-outcome-ok",
            held: &[("outcome-west", "us-west-2"), ("outcome-east", "us-east-1")],
            associated: &["outcome-west", "outcome-east"],
            public_access_block: None,
            region: REGION,
            account: ACCOUNT,
            failure: None,
        },
        OutcomeRow {
            case: "one held bucket, with a public-access block supplied",
            name: "outcome-ok-blocked-mrap",
            client_token: "t-outcome-ok-blocked",
            held: &[("outcome-solo", REGION)],
            associated: &["outcome-solo"],
            public_access_block: Some([true, false, false, true]),
            region: REGION,
            account: ACCOUNT,
            failure: None,
        },
        OutcomeRow {
            case: "the same bucket named twice, one after the other",
            name: "outcome-twice-mrap",
            client_token: "t-outcome-twice",
            held: &[("outcome-twice-bucket", REGION)],
            associated: &["outcome-twice-bucket", "outcome-twice-bucket"],
            public_access_block: None,
            region: REGION,
            account: ACCOUNT,
            failure: Some((REQUEST_INVALID_CODE, DUPLICATE_ASSOCIATED_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "the same bucket named twice with another between them",
            name: "outcome-apart-mrap",
            client_token: "t-outcome-apart",
            held: &[
                ("outcome-apart-first", REGION),
                ("outcome-apart-other", "eu-west-1"),
            ],
            associated: &[
                "outcome-apart-first",
                "outcome-apart-other",
                "outcome-apart-first",
            ],
            public_access_block: Some([true, true, true, true]),
            region: REGION,
            account: ACCOUNT,
            failure: Some((REQUEST_INVALID_CODE, DUPLICATE_ASSOCIATED_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "one bucket named three times, signed for another Region",
            name: "outcome-thrice-mrap",
            client_token: "t-outcome-thrice",
            held: &[("outcome-thrice-bucket", OTHER_REGION)],
            associated: &[
                "outcome-thrice-bucket",
                "outcome-thrice-bucket",
                "outcome-thrice-bucket",
            ],
            public_access_block: None,
            region: OTHER_REGION,
            account: ACCOUNT,
            failure: Some((REQUEST_INVALID_CODE, DUPLICATE_ASSOCIATED_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "the only named bucket is one the store does not hold",
            name: "outcome-absent-mrap",
            client_token: "t-outcome-absent",
            held: &[],
            associated: &["outcome-absent-bucket"],
            public_access_block: None,
            region: REGION,
            account: ACCOUNT,
            failure: Some((NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "a bucket the store does not hold, after two it holds",
            name: "outcome-absent-last-mrap",
            client_token: "t-outcome-absent-last",
            held: &[
                ("outcome-held-west", "us-west-2"),
                ("outcome-held-sa", "sa-east-1"),
            ],
            associated: &[
                "outcome-held-west",
                "outcome-held-sa",
                "outcome-absent-tail",
            ],
            public_access_block: Some([false, false, false, false]),
            region: REGION,
            account: ACCOUNT,
            failure: Some((NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "a bucket the store does not hold, before one it holds",
            name: "outcome-absent-head-mrap",
            client_token: "t-outcome-absent-head",
            held: &[("outcome-held-ap", "ap-southeast-2")],
            associated: &["outcome-absent-head", "outcome-held-ap"],
            public_access_block: None,
            region: OTHER_REGION,
            account: ACCOUNT,
            failure: Some((NO_SUCH_BUCKET_CODE, NO_SUCH_BUCKET_MESSAGE)),
        },
        OutcomeRow {
            case: "a repeated bucket and an absent one in one request",
            name: "outcome-both-mrap",
            client_token: "t-outcome-both",
            held: &[("outcome-both-held", REGION)],
            associated: &[
                "outcome-both-held",
                "outcome-both-held",
                "outcome-both-absent",
            ],
            public_access_block: None,
            region: REGION,
            account: ACCOUNT,
            // The list itself is read before the buckets it names are
            // resolved, so the repeat is what this request cannot get past.
            failure: Some((REQUEST_INVALID_CODE, DUPLICATE_ASSOCIATED_BUCKET_MESSAGE)),
        },
    ]
}

/// Submit `row`'s create WITHOUT provisioning anything: which buckets the
/// object store holds is the row's own, recorded by the caller.
async fn submit(emulator: &Emulator, case: &CreateCase) -> Reply {
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

/// `GetMultiRegionAccessPoint` for `name`.
async fn get_access_point(emulator: &Emulator, region: &str, account: &str, name: &str) -> Reply {
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

/// The asynchronous request `token` names, once the clock owes it an outcome.
///
/// Polls the describe until the request is no longer in progress — the
/// CONDITION this suite waits on — or until a deadline far beyond the span the
/// service holds a request for has passed.
async fn settled_operation(
    emulator: &Emulator,
    case: &str,
    region: &str,
    account: &str,
    token: &str,
) -> Reply {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe(emulator, region, account, token).await;
        let status = leaf(&leaves_of(&described.body), "AsyncOperation/RequestStatus");
        if status.as_deref() != Some(ASYNC_REQUEST_IN_PROGRESS) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the clock to owe the request an outcome, got body {}",
            described.body
        );
        tokio::time::sleep(SETTLE_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_create_over_buckets_it_cannot_associate_fails_its_request_and_publishes_nothing() {
    let emulator = Emulator::start();
    let rows = outcome_rows();

    // Every request is submitted before any outcome is read, so the clock —
    // not a later call — is what settles them.
    let mut submitted = Vec::new();
    for row in &rows {
        for (bucket, region) in row.held {
            emulator.provision_bucket_in(row.case, bucket, region);
        }
        let case = row.create_case();
        let created = submit(&emulator, &case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the create to be accepted with 200, got {} with body {}",
            row.case, created.status, created.body
        );
        let token = created.text("RequestTokenARN").unwrap_or_default();
        assert_token_shape(row.case, &token, row.region, "create");
        submitted.push(token);
    }

    for (row, token) in rows.iter().zip(&submitted) {
        let described =
            settled_operation(&emulator, row.case, row.region, row.account, token).await;
        let leaves = leaves_of(&described.body);
        let expected_status = match row.failure {
            Some(_) => ASYNC_REQUEST_FAILED,
            None => ASYNC_REQUEST_SUCCEEDED,
        };
        assert_eq!(
            leaf(&leaves, "AsyncOperation/RequestStatus"),
            Some(expected_status.to_string()),
            "case {:?}: expected the settled request status, got body {}",
            row.case,
            described.body
        );

        let details = ["Code", "Resource", "Message"].map(|member| {
            leaf(
                &leaves,
                &format!("AsyncOperation/ResponseDetails/ErrorDetails/{member}"),
            )
        });
        let expected_details = row.failure.map(|(code, message)| {
            vec![
                code.to_string(),
                row.name.to_string(),
                message.to_string(),
            ]
        });
        assert_eq!(
            details.iter().cloned().collect::<Option<Vec<String>>>(),
            expected_details,
            "case {:?}: expected the error details of the outcome, got body {}",
            row.case,
            described.body
        );
        // The failure names the request it was the outcome of, in the shape an
        // S3-family service renders a request id, and names the same one on
        // every later read.
        let request_id = leaf(
            &leaves,
            "AsyncOperation/ResponseDetails/ErrorDetails/RequestId",
        );
        assert_eq!(
            request_id.as_ref().map(|id| (
                id.len(),
                id.chars()
                    .all(|character| character.is_ascii_digit()
                        || character.is_ascii_uppercase() && character.is_ascii_hexdigit())
            )),
            row.failure.map(|_| (16, true)),
            "case {:?}: expected the failure to name its request, got body {}",
            row.case,
            described.body
        );
        let again = describe(&emulator, row.region, row.account, token).await;
        assert_eq!(
            leaf(
                &leaves_of(&again.body),
                "AsyncOperation/ResponseDetails/ErrorDetails/RequestId"
            ),
            request_id,
            "case {:?}: expected the failure to name the same request on a second read, got \
             body {}",
            row.case,
            again.body
        );

        // A request that FAILED carried none of its work out, so it reports no
        // per-Region result; one that succeeded reports one per associated
        // Region.
        let regional: Vec<String> = leaves
            .iter()
            .filter(|(path, _)| {
                path == "AsyncOperation/ResponseDetails/MultiRegionAccessPointDetails/\
                         Regions/Region/RequestStatus"
            })
            .map(|(_, value)| value.clone())
            .collect();
        let expected_regional = match row.failure {
            Some(_) => Vec::new(),
            None => vec![ASYNC_REQUEST_SUCCEEDED.to_string(); row.associated.len()],
        };
        assert_eq!(
            regional, expected_regional,
            "case {:?}: expected the per-Region results of the outcome, got body {}",
            row.case, described.body
        );

        // The parameters of the request are reported whatever became of it,
        // carrying the public-access block in force for the access point it
        // asked for.
        assert_eq!(
            PUBLIC_ACCESS_BLOCK_MEMBERS
                .iter()
                .map(|member| leaf(
                    &leaves,
                    &format!(
                        "AsyncOperation/RequestParameters/\
                         CreateMultiRegionAccessPointRequest/PublicAccessBlock/{member}"
                    )
                ))
                .collect::<Option<Vec<String>>>(),
            Some(row.effective_public_access_block()),
            "case {:?}: expected the request parameters to carry the effective public-access \
             block, got body {}",
            row.case,
            described.body
        );

        // What the create left behind: an access point for the request that
        // was carried out, and nothing at all for the one that was not.
        let read = get_access_point(&emulator, row.region, row.account, row.name).await;
        match row.failure {
            Some(_) => {
                assert_eq!(
                    (read.status, read.code().as_str()),
                    (404, NO_SUCH_MRAP_CODE),
                    "case {:?}: expected the failed create to have published nothing, got {} \
                     with body {}",
                    row.case,
                    read.status,
                    read.body
                );
                assert_eq!(
                    read.text("Message").as_deref(),
                    Some(NO_SUCH_MRAP_MESSAGE),
                    "case {:?}: expected the absent-access-point message, got body {}",
                    row.case,
                    read.body
                );
                assert_eq!(
                    (
                        read.text("MultiRegionAccessPointName"),
                        read.text("AccountId")
                    ),
                    (Some(row.name.to_string()), Some(row.account.to_string())),
                    "case {:?}: expected the refusal to name the access point and the account, \
                     got body {}",
                    row.case,
                    read.body
                );
            }
            None => {
                assert_eq!(
                    read.status, 200,
                    "case {:?}: expected the created access point to be readable, got {} with \
                     body {}",
                    row.case, read.status, read.body
                );
                let leaves = leaves_of(&read.body);
                assert_eq!(
                    leaf(&leaves, "AccessPoint/Status"),
                    Some(STATUS_READY.to_string()),
                    "case {:?}: expected the settled create status, got body {}",
                    row.case,
                    read.body
                );
                assert_eq!(
                    PUBLIC_ACCESS_BLOCK_MEMBERS
                        .iter()
                        .map(|member| leaf(
                            &leaves,
                            &format!("AccessPoint/PublicAccessBlock/{member}")
                        ))
                        .collect::<Option<Vec<String>>>(),
                    Some(row.effective_public_access_block()),
                    "case {:?}: expected the access point to carry the same effective block, \
                     got body {}",
                    row.case,
                    read.body
                );
            }
        }
    }

    // Nothing a failed create named is in the account's listing either.
    let listed = call_in_region(
        &emulator.router,
        REGION,
        "GET",
        "/v20180820/mrap/instances",
        ACCOUNT,
        "",
    )
    .await;
    for row in rows.iter().filter(|row| row.failure.is_some()) {
        assert!(
            !listed.body.contains(&format!("<Name>{}</Name>", row.name)),
            "case {:?}: expected the failed create to leave no access point to list, got body {}",
            row.case,
            listed.body
        );
    }
}

/// A name no Multi-Region Access Point may be given, and which rule of the
/// modelled `MultiRegionAccessPointName` it breaks. The pattern
/// (`^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`) and the length (`0..50`) come from
/// `smithy_query(query="operation", operation="GetMultiRegionAccessPoint")`.
struct InvalidName {
    case: &'static str,
    name: String,
    /// Whether the name can also be submitted as one segment of a request
    /// PATH: a name that is empty addresses the listing instead, and one
    /// carrying a space is no path segment at all.
    addressable: bool,
}

/// One row per rule of the name a Multi-Region Access Point may be given, plus
/// the character classes the pattern excludes — the upper-case row is the shape
/// a sim-to-real probe sends, and it is one row of the set rather than the set.
fn invalid_names() -> Vec<InvalidName> {
    vec![
        InvalidName {
            case: "an upper-case character",
            name: "sim2real-Probe-s3c".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "every character upper-case",
            name: "MRAPNAME".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "one character past the modelled maximum length",
            name: "a".repeat(51),
            addressable: true,
        },
        InvalidName {
            case: "one character short of the shortest the pattern admits",
            name: "ab".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "a single character",
            name: "a".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "no name at all",
            name: String::new(),
            addressable: false,
        },
        InvalidName {
            case: "a leading hyphen",
            name: "-mrapname".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "a trailing hyphen",
            name: "mrapname-".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "an underscore",
            name: "mrap_name".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "a dot, which the alias pattern admits and the name does not",
            name: "mrap.name".to_string(),
            addressable: true,
        },
        InvalidName {
            case: "a space",
            name: "mrap name".to_string(),
            addressable: false,
        },
    ]
}

/// The request document a create submits for `name`, over a bucket the row
/// provisions.
fn create_body(name: &str, client_token: &str, bucket: &str) -> String {
    format!(
        "<CreateMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name>\
         <Regions><Region><Bucket>{}</Bucket></Region></Regions></Details>\
         </CreateMultiRegionAccessPointRequest>",
        xml_escape(client_token),
        xml_escape(name),
        xml_escape(bucket)
    )
}

/// The request document a delete submits for `name`.
fn delete_body(name: &str, client_token: &str) -> String {
    format!(
        "<DeleteMultiRegionAccessPointRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name></Details>\
         </DeleteMultiRegionAccessPointRequest>",
        xml_escape(client_token),
        xml_escape(name)
    )
}

/// The request document a policy put submits for `name`.
fn put_policy_body(name: &str, client_token: &str) -> String {
    let policy = r#"{"Version":"2012-10-17","Statement":[]}"#;
    format!(
        "<PutMultiRegionAccessPointPolicyRequest xmlns=\"{XMLNS}\">\
         <ClientToken>{}</ClientToken><Details><Name>{}</Name><Policy>{}</Policy></Details>\
         </PutMultiRegionAccessPointPolicyRequest>",
        xml_escape(client_token),
        xml_escape(name),
        xml_escape(policy)
    )
}

/// Assert that `reply` is the refusal of a Multi-Region Access Point name this
/// service will not accept: its own code and status, the one fixed sentence
/// about the name, and the pair naming the request it answers.
fn assert_name_refused(case: &str, name: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code().as_str()),
        (400, REQUEST_INVALID_CODE),
        "case {case:?}: expected the invalid-name refusal, got {} with body {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(INVALID_MULTI_REGION_ACCESS_POINT_NAME_MESSAGE),
        "case {case:?}: expected the fixed invalid-name message, got body {}",
        reply.body
    );
    assert!(
        name.len() < 4
            || !reply
                .text("Message")
                .is_some_and(|message| message.contains(name)),
        "case {case:?}: expected the refusal not to restate the submitted name, got body {}",
        reply.body
    );
    for element in ["RequestId", "HostId"] {
        assert!(
            reply
                .text(element)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the refusal to carry a {element}, got body {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_name_no_access_point_may_be_given_is_refused_alike_on_every_route_that_carries_one() {
    let emulator = Emulator::start();
    emulator.provision_bucket("the bucket every create names", "name-rule-bucket");

    for (index, row) in invalid_names().into_iter().enumerate() {
        // Every route whose request DOCUMENT carries the name.
        let documents = [
            (
                "CreateMultiRegionAccessPoint",
                "/v20180820/async-requests/mrap/create",
                create_body(&row.name, &format!("t-name-{index}"), "name-rule-bucket"),
            ),
            (
                "DeleteMultiRegionAccessPoint",
                "/v20180820/async-requests/mrap/delete",
                delete_body(&row.name, &format!("t-name-del-{index}")),
            ),
            (
                "PutMultiRegionAccessPointPolicy",
                "/v20180820/async-requests/mrap/put-policy",
                put_policy_body(&row.name, &format!("t-name-put-{index}")),
            ),
        ];
        for (operation, path, body) in documents {
            let reply = call_in_region(&emulator.router, REGION, "POST", path, ACCOUNT, &body).await;
            assert_name_refused(&format!("{}: {operation}", row.case), &row.name, &reply);
        }

        if !row.addressable {
            continue;
        }
        // And every route that carries it in its PATH.
        for (operation, suffix) in [
            ("GetMultiRegionAccessPoint", ""),
            ("GetMultiRegionAccessPointPolicy", "/policy"),
            ("GetMultiRegionAccessPointPolicyStatus", "/policystatus"),
        ] {
            let reply = call_in_region(
                &emulator.router,
                REGION,
                "GET",
                &format!("/v20180820/mrap/instances/{}{suffix}", row.name),
                ACCOUNT,
                "",
            )
            .await;
            assert_name_refused(&format!("{}: {operation}", row.case), &row.name, &reply);
        }
    }
}

#[tokio::test]
async fn a_name_the_model_admits_is_not_refused_as_an_invalid_name() {
    let emulator = Emulator::start();
    emulator.provision_bucket("the bucket every create names", "name-bound-bucket");

    // Both ends of the admitted length, and the character classes the pattern
    // admits in between.
    let admitted = [
        ("the shortest name the pattern admits", "abc".to_string()),
        ("the longest name the model admits", "a".repeat(50)),
        ("digits and hyphens at both ends", "7-mrap-9".to_string()),
    ];
    for (index, (case, name)) in admitted.into_iter().enumerate() {
        let created = call_in_region(
            &emulator.router,
            REGION,
            "POST",
            "/v20180820/async-requests/mrap/create",
            ACCOUNT,
            &create_body(&name, &format!("t-admitted-{index}"), "name-bound-bucket"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected an admitted name to be accepted, got {} with body {}",
            created.status, created.body
        );

        // A name the model admits that nothing was created under is a MISSING
        // access point, not an invalid name: the two refusals are what tell a
        // caller which of them to fix.
        let absent = get_access_point(&emulator, REGION, ACCOUNT, "never-created-mrap").await;
        assert_eq!(
            (absent.status, absent.code().as_str()),
            (404, NO_SUCH_MRAP_CODE),
            "case {case:?}: expected an admitted but unknown name refused as absent, got {} \
             with body {}",
            absent.status,
            absent.body
        );
    }
}
