// SPDX-License-Identifier: CC0-1.0

//! The two paging rules EVERY paginated reader of this service shares, driven over the wire.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what these tests prove
//! is what a client sees.
//!
//! The rules, which `crate::state::page_size` and `crate::state::offset_page` state once for
//! the whole crate:
//!
//! * HOW LARGE a page is — the size the request asks for, the reader's own default when it
//!   asks for none (an absent `maxResults`, or the modelled range's lower bound `0`, which
//!   the service documents no meaning for), and never more than the reader's own ceiling.
//! * WHICH items that page carries and what follows it — the items after the point the
//!   previous page stopped at, with a `nextToken` handed out exactly when the sequence
//!   continues past the page, resuming where it stopped.
//! * WHICH WALK a page token belongs to — a `nextToken` names a boundary of the walk that
//!   minted it and of no other, so a reader resumes its own token and refuses one another
//!   reader minted, or one minted for a different selection of its own, rather than answering
//!   it with a page of a different result set. The Region a request is signed for is part of
//!   that selection wherever it scopes what the reader answers, so a token minted at one
//!   regional endpoint does not resume a walk at another.
//! * WHAT a selection IS — the set the request's members NAME, not the way they were spelled:
//!   a selecting member a request leaves out and the empty value it can send instead name the
//!   same set, because every reader reads such a member defaulted, so either spelling's token
//!   resumes the other's walk instead of being refused as a token of another selection.
//! * HOW a reader refuses a token it cannot resume — with the ONE sentence this crate words
//!   for that condition (`crate::state::INVALID_NEXT_TOKEN_MESSAGE`), identical at every
//!   reader, so a client walking two readers of this service with a token neither can resume
//!   reads the same refusal from both rather than a sentence per operation.
//!
//! Both are asserted over a TABLE of every reader of this service whose page the request
//! sizes, each with its own default and ceiling (the eleven rows below), crossed with the
//! page sizes the rule generalises over: none at all, the range's lower bound, one item, one
//! short of the sequence, exactly the sequence, one past it, and the reader's own ceiling.
//! Nothing here asserts a page size the recorded calls happened to use; what is asserted is
//! that no reader answers a page size differently from the others.
//!
//! `GetRecommendationSummaries` is NOT one of the rows below, because the sequence it walks is
//! the `RecommendationSourceType` vocabulary the service reports on rather than records the
//! account's resources grow, so its cursor names the GROUP a page resumes at instead of an
//! offset into a selection; it applies the same page-size rule, pinned over its own table of
//! sizes in `tests/all/summary_block_sets.rs`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, MemberAccountEnrollment, DEFAULT_PAGE_SIZE,
    ENROLLMENT_STATUSES_MAX_PAGE_SIZE, IDLE_MAX_PAGE_SIZE, INVALID_NEXT_TOKEN_MESSAGE,
    MAX_PAGE_SIZE, ORDER_DESC, RESOURCE_TYPE_EBS_VOLUME, SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION,
    SCOPE_RESOURCE_ARN, SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_ecs, call_in_region, create_bucket, emulator_with_analysed_services_and_engine,
    error_code, instance_arn, make_analysis_due, opt_in, put_preference,
    register_analysed_functions, seed_cloudwatch_series, ACCOUNT, REGION,
};

/// The destination bucket the export jobs below are created against. Compute Optimizer does
/// not create it; the request just names it.
const BUCKET: &str = "shared-paging-rules";

/// The cluster the analysed Amazon ECS services below run in.
const CLUSTER: &str = "paging-rules";

/// One paginated reader of this service: how to call it, where its page lives in the
/// response, what identifies an item of that page, and the two numbers it applies the shared
/// page-size rule with.
struct PagedReader {
    /// The operation name, as `X-Amz-Target` carries it.
    operation: &'static str,
    /// The request members other than `maxResults` and `nextToken`.
    request: Value,
    /// The output member the page is reported in.
    items: &'static str,
    /// The path to the member of an item that identifies it, so two reads can be compared
    /// item by item without depending on anything a clock derives.
    identity: &'static [&'static str],
    /// The page size this reader uses when the request asks for none.
    default_page: usize,
    /// The largest page this reader will build, whatever the request asks for.
    cap: usize,
}

/// Every reader of this service whose page a request sizes, each with the default and the
/// ceiling it applies the shared page-size rule with: nine of them page at the service's own
/// default under the model's 0..1000 ceiling, while `GetIdleRecommendations` and
/// `GetEnrollmentStatusesForOrganization` are bounded lower by what their own members
/// document.
fn readers() -> Vec<PagedReader> {
    vec![
        PagedReader {
            operation: "DescribeRecommendationExportJobs",
            request: json!({}),
            items: "recommendationExportJobs",
            identity: &["jobId"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetAutoScalingGroupRecommendations",
            request: json!({}),
            items: "autoScalingGroupRecommendations",
            identity: &["autoScalingGroupArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEBSVolumeRecommendations",
            request: json!({}),
            items: "volumeRecommendations",
            identity: &["volumeArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEC2InstanceRecommendations",
            request: json!({}),
            items: "instanceRecommendations",
            identity: &["instanceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetECSServiceRecommendations",
            request: json!({}),
            items: "ecsServiceRecommendations",
            identity: &["serviceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetEnrollmentStatusesForOrganization",
            request: json!({}),
            items: "accountEnrollmentStatuses",
            identity: &["accountId"],
            default_page: ENROLLMENT_STATUSES_MAX_PAGE_SIZE,
            cap: ENROLLMENT_STATUSES_MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetIdleRecommendations",
            request: json!({}),
            items: "idleRecommendations",
            identity: &["resourceArn"],
            default_page: IDLE_MAX_PAGE_SIZE,
            cap: IDLE_MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetLambdaFunctionRecommendations",
            request: json!({}),
            items: "lambdaFunctionRecommendations",
            identity: &["functionArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetLicenseRecommendations",
            request: json!({}),
            items: "licenseRecommendations",
            identity: &["resourceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetRDSDatabaseRecommendations",
            request: json!({}),
            items: "rdsDBRecommendations",
            identity: &["resourceArn"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
        PagedReader {
            operation: "GetRecommendationPreferences",
            request: json!({"resourceType": "Ec2Instance"}),
            items: "recommendationPreferencesDetails",
            identity: &["scope", "value"],
            default_page: DEFAULT_PAGE_SIZE,
            cap: MAX_PAGE_SIZE,
        },
    ]
}

/// A page size the request asks for, and the page it must be answered with.
#[derive(Debug)]
struct PageSizeCase {
    /// What the row is, for the assertion message.
    name: String,
    /// The `maxResults` the request carries, or `None` for a request naming none.
    max_results: Option<usize>,
    /// The items the first page must carry, by identity.
    expected: Vec<String>,
    /// Whether a `nextToken` must come with that page.
    expect_token: bool,
}

/// Whatever page size a request asks for, every reader answers it by the same rule: the size
/// asked for when it asks for one, the reader's own default when it asks for none, and never
/// more than the reader's own ceiling.
#[tokio::test]
async fn the_page_size_rule_is_the_same_for_every_paginated_reader() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        let count = whole.len();
        assert!(
            count >= 2,
            "case {:?}: this fixture must hold at least two items for a page size to be \
             observable, got {whole:?}",
            reader.operation
        );
        assert!(
            count <= reader.default_page,
            "case {:?}: this fixture must fit one default-sized page of {}, got {count}",
            reader.operation,
            reader.default_page
        );

        // The rule generalises over the size asked for, so the rows span the forms that size
        // can take: none at all, the modelled range's lower bound (no size asked for), one
        // item, one short of the sequence, exactly it, one past it, and the ceiling.
        let cases = vec![
            PageSizeCase {
                name: "no maxResults at all".to_string(),
                max_results: None,
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: "the modelled range's lower bound 0".to_string(),
                max_results: Some(0),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: "one item per page".to_string(),
                max_results: Some(1),
                expected: whole[..1].to_vec(),
                expect_token: true,
            },
            PageSizeCase {
                name: format!("one short of the sequence ({})", count - 1),
                max_results: Some(count - 1),
                expected: whole[..count - 1].to_vec(),
                expect_token: true,
            },
            PageSizeCase {
                name: format!("exactly the sequence ({count})"),
                max_results: Some(count),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: format!("one past the sequence ({})", count + 1),
                max_results: Some(count + 1),
                expected: whole.clone(),
                expect_token: false,
            },
            PageSizeCase {
                name: format!("the reader's own ceiling ({})", reader.cap),
                max_results: Some(reader.cap),
                expected: whole.clone(),
                expect_token: false,
            },
        ];

        for case in cases {
            let label = format!("{} / {}", reader.operation, case.name);
            let body = read(&router, &reader, &label, case.max_results, None).await;
            let page = page_of(&body, &reader);
            assert_eq!(
                page, case.expected,
                "case {label:?}: expected the page {:?}, got {page:?}",
                case.expected
            );
            assert!(
                page.len() <= reader.cap,
                "case {label:?}: a page may hold at most {} items, got {}",
                reader.cap,
                page.len()
            );
            assert_eq!(
                body["nextToken"].as_str().is_some(),
                case.expect_token,
                "case {label:?}: expected a nextToken: {}, got {body}",
                case.expect_token
            );
        }
    }
}

/// Whatever page size walks a reader's sequence, the walk reports that reader's unpaged
/// sequence exactly once, and only its final page comes without a token.
#[tokio::test]
async fn a_paged_walk_reports_the_unpaged_sequence_exactly_once() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        let count = whole.len();

        // Rows: every page size that splits the sequence differently — one item at a time,
        // two at a time, and the whole sequence in one page.
        for page_size in [1, 2, count] {
            let label = format!("{} / maxResults={page_size}", reader.operation);
            let mut walked: Vec<String> = Vec::new();
            let mut pages: Vec<usize> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let body = read(&router, &reader, &label, Some(page_size), token.as_deref()).await;
                let page = page_of(&body, &reader);
                assert!(
                    page.len() <= page_size,
                    "case {label:?}: a page may hold at most {page_size} items, got {}",
                    page.len()
                );
                pages.push(page.len());
                walked.extend(page);
                token = body["nextToken"].as_str().map(ToString::to_string);
                let remaining = count - walked.len();
                assert_eq!(
                    token.is_some(),
                    remaining > 0,
                    "case {label:?}: with {remaining} items left, expected a nextToken: {}, got \
                     {body}",
                    remaining > 0
                );
                if token.is_none() {
                    break;
                }
                assert!(
                    pages.len() <= count,
                    "case {label:?}: the walk does not terminate, pages so far {pages:?}"
                );
            }
            assert_eq!(
                walked, whole,
                "case {label:?}: a paged walk must report the unpaged sequence, got {walked:?}"
            );
        }
    }
}

/// A `nextToken` resumes the WALK that minted it and no other.
///
/// Every reader of this service hands out an opaque token, and several of them encode the same
/// kind of boundary, so what makes a token this reader's is the operation and the selection
/// bound into it. Asserted over the whole table crossed with itself: each reader resumes its
/// OWN token, and refuses every other reader's with the one refusal this service models for a
/// value it cannot honour — never with a page of a different result set, and never with an
/// empty final page a client would read as "the walk is finished".
#[tokio::test]
async fn a_next_token_resumes_only_the_walk_that_minted_it() {
    let (router, _engine) = paged_emulator().await;

    // One token per reader, minted by a one-item page of its own walk: the fixture gives
    // every row more than one item to report, so each hands one out.
    let mut minted: Vec<(&'static str, String)> = Vec::new();
    for reader in readers() {
        let body = read(&router, &reader, "minting a token", Some(1), None).await;
        let token = body["nextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: a one-item page of a longer sequence must hand out a nextToken, got \
                 {body}",
                reader.operation
            )
        });
        minted.push((reader.operation, token.to_string()));
    }

    for reader in readers() {
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        assert!(
            whole.len() >= 2,
            "case {:?}: this fixture must hold at least two items for a token to resume at one, \
             got {whole:?}",
            reader.operation
        );
        let own = minted
            .iter()
            .find(|(operation, _)| *operation == reader.operation)
            .map(|(_, token)| token.clone())
            .expect("every reader of the table minted a token");

        // Its OWN token resumes its own walk, at the item after the page that minted it.
        let label = format!("{} resuming its own token", reader.operation);
        let page = page_of(
            &read(&router, &reader, &label, Some(1), Some(&own)).await,
            &reader,
        );
        assert_eq!(
            page,
            whole[1..2].to_vec(),
            "case {label:?}: expected the page {:?}, got {page:?}",
            &whole[1..2]
        );

        // Every OTHER reader's token names a boundary of a walk this reader is not.
        for (minted_by, token) in &minted {
            if *minted_by == reader.operation {
                continue;
            }
            let label = format!("{} resuming a token {minted_by} minted", reader.operation);
            let mut request = reader.request.clone();
            request["nextToken"] = json!(token);
            let (status, body) = call(&router, reader.operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected {request} to be refused with 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {label:?}: expected InvalidParameterValueException, got {body}"
            );
        }
    }
}

/// The forms a value that is not a token of the walk it was handed to arrives in: not a token
/// at all, the empty value (`NextToken` carries no length constraint —
/// `smithy_query(query="shape",shape="NextToken")`), a value that decodes as this service's
/// own hex encoding but names no boundary (`7b7d`, the empty JSON object), one that decodes to
/// bytes that are no payload of this service at all (`deadbeef`), and one hex decoding itself
/// refuses because its length is odd (`0`). The rows differ in the FORM of the value, because
/// what makes a token unresumable is that the walk never minted it, not how it looks.
const UNRESUMABLE_TOKENS: &[&str] = &["not-a-token", "", "7b7d", "deadbeef", "0"];

/// The request members that scope a reader's selection, which the refusal of a token does NOT
/// name: the fingerprint a token carries is this service's business, so the sentence a reader
/// answers with cannot vary with — or disclose — the members that made it up.
const SELECTION_MEMBER_NAMES: &[&str] = &[
    "resourceArns",
    "accountIds",
    "filters",
    "orderBy",
    "recommendationPreferences",
    "jobIds",
    "resourceType",
    "scope",
];

/// Assert `body` is the one refusal this service answers a `nextToken` it cannot resume with:
/// `400 InvalidParameterValueException` carrying the crate's single sentence, naming neither
/// the operation that refused nor any member of the selection the token was fingerprinted
/// over, and reporting no page at all.
fn assert_one_invalid_token_refusal(
    case: &str,
    operation: &str,
    status: u16,
    body: &Value,
    items: &str,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the token to be refused with 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, INVALID_NEXT_TOKEN_MESSAGE,
        "case {case:?}: expected the crate's one sentence {INVALID_NEXT_TOKEN_MESSAGE:?}, got \
         {message:?}"
    );
    assert!(
        !message.contains(operation),
        "case {case:?}: the refusal must not name the operation that answered it, got {message:?}"
    );
    for member in SELECTION_MEMBER_NAMES {
        assert!(
            !message.contains(member),
            "case {case:?}: the refusal must not name the selection member {member:?}, got \
             {message:?}"
        );
    }
    assert!(
        body[items].is_null(),
        "case {case:?}: a refused read reports no page at all, got {body}"
    );
}

/// Every paginated reader of this service, as the operation plus the request members it needs
/// besides `maxResults` and `nextToken` and the output member it reports its page in: the
/// table above, plus `GetRecommendationSummaries`, which is absent from it because its cursor
/// names the GROUP a page resumes at rather than an offset into a selection, yet resolves a
/// `nextToken` a request carries by the same rule.
fn token_reading_surfaces() -> Vec<(&'static str, Value, &'static str)> {
    let mut surfaces: Vec<(&'static str, Value, &'static str)> = readers()
        .iter()
        .map(|reader| (reader.operation, reader.request.clone(), reader.items))
        .collect();
    surfaces.push((
        "GetRecommendationSummaries",
        json!({}),
        "recommendationSummaries",
    ));
    surfaces
}

/// A `nextToken` no walk of this service minted is refused with the SAME sentence by every
/// reader that reads one, whatever form the value arrived in: one wording for the whole crate,
/// so two readers never refuse the identical condition differently.
#[tokio::test]
async fn an_unresumable_next_token_is_refused_with_one_wording_by_every_reader() {
    let (router, _engine) = paged_emulator().await;

    for (operation, base, items) in token_reading_surfaces() {
        for token in UNRESUMABLE_TOKENS {
            let case = format!("{operation} handed {token:?}");
            let mut request = base.clone();
            request["nextToken"] = json!(token);
            let (status, body) = call(&router, operation, request).await;
            assert_one_invalid_token_refusal(&case, operation, status, &body, items);
        }
    }
}

/// A token ANOTHER reader minted — a value this service did issue, for a walk the reader
/// handed it is not — is refused with that same one sentence, and the reader's own token still
/// resumes its walk, so what is pinned is the wording of a refusal rather than a reader that
/// refuses every token.
#[tokio::test]
async fn a_foreign_readers_token_is_refused_with_the_same_one_wording() {
    let (router, _engine) = paged_emulator().await;

    // One token per reader, minted by a one-item page of its own walk.
    let mut minted: Vec<(&'static str, String)> = Vec::new();
    for reader in readers() {
        let body = read(&router, &reader, "minting a token", Some(1), None).await;
        let token = body["nextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {:?}: a one-item page of a longer sequence must hand out a nextToken, got \
                 {body}",
                reader.operation
            )
        });
        minted.push((reader.operation, token.to_string()));
    }

    for (operation, base, items) in token_reading_surfaces() {
        for (minted_by, token) in &minted {
            if *minted_by == operation {
                continue;
            }
            let case = format!("{operation} handed a token {minted_by} minted");
            let mut request = base.clone();
            request["nextToken"] = json!(token);
            let (status, body) = call(&router, operation, request).await;
            assert_one_invalid_token_refusal(&case, operation, status, &body, items);
        }
    }

    // The control: every minted token still resumes the walk that minted it.
    for reader in readers() {
        let own = minted
            .iter()
            .find(|(operation, _)| *operation == reader.operation)
            .map(|(_, token)| token.clone())
            .expect("every reader of the table minted a token");
        let label = format!("{} resuming its own token", reader.operation);
        let body = read(&router, &reader, &label, Some(1), Some(&own)).await;
        assert!(
            body[reader.items].is_array(),
            "case {label:?}: a reader's own token resumes its walk, got {body}"
        );
    }
}

/// The member each recommendation reader narrows its selection with, from that operation's
/// input in this service's Smithy model (`GetAutoScalingGroupRecommendations$autoScalingGroupArns`,
/// `GetEBSVolumeRecommendations$volumeArns`, `GetEC2InstanceRecommendations$instanceArns`,
/// `GetECSServiceRecommendations$serviceArns`, `GetLambdaFunctionRecommendations$functionArns`).
/// A reader absent from this list narrows its selection by no resource ARN at all.
const RESOURCE_SELECTION_MEMBERS: &[(&str, &str)] = &[
    (
        "GetAutoScalingGroupRecommendations",
        "autoScalingGroupArns",
    ),
    ("GetEBSVolumeRecommendations", "volumeArns"),
    ("GetEC2InstanceRecommendations", "instanceArns"),
    ("GetECSServiceRecommendations", "serviceArns"),
    ("GetLambdaFunctionRecommendations", "functionArns"),
];

/// A `nextToken` resumes the SELECTION it was minted for and no other: a token of a reader's
/// own walk, handed back with a different set of resources selected, is refused rather than
/// resolved against a sequence it names no position in. The rows differ in how the selection
/// changed — the same resources named explicitly, and a narrower set — because what makes the
/// token foreign is the selection it was minted for, not how much of it survives.
#[tokio::test]
async fn a_next_token_does_not_resume_a_different_selection() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let Some((_, member)) = RESOURCE_SELECTION_MEMBERS
            .iter()
            .find(|(operation, _)| *operation == reader.operation)
        else {
            continue;
        };
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        assert!(
            whole.len() >= 2,
            "case {:?}: this fixture must hold at least two items for a token to resume at one, \
             got {whole:?}",
            reader.operation
        );
        let body = read(&router, &reader, "minting a token", Some(1), None).await;
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a one-item page of a longer sequence must hand out a nextToken, \
                     got {body}",
                    reader.operation
                )
            })
            .to_string();

        for (case, selected) in [
            ("the same resources named explicitly", whole.clone()),
            ("one resource of the sequence", whole[..1].to_vec()),
        ] {
            let label = format!("{} resuming its token with {case}", reader.operation);
            let mut request = reader.request.clone();
            request["nextToken"] = json!(token);
            request[*member] = json!(selected);
            let (status, body) = call(&router, reader.operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected {request} to be refused with 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {label:?}: expected InvalidParameterValueException, got {body}"
            );
        }
    }
}

/// The preference reader pages a SELECTED set of stored records too, and its selection is
/// `resourceType` plus the optional `scope` rather than a list of resource ARNs — so the same
/// rule holds there: a token minted for one selection of records is refused by another, never
/// applied to it. The control row is the selection the token WAS minted for, which resumes.
#[tokio::test]
async fn a_preference_token_resumes_only_the_records_it_was_minted_for() {
    let (router, _engine) = paged_emulator().await;
    let operation = "GetRecommendationPreferences";
    let selection = json!({"resourceType": "Ec2Instance"});

    let mut request = selection.clone();
    request["maxResults"] = json!(1);
    let (status, body) = call(&router, operation, request).await;
    assert_eq!(
        status, 200,
        "minting a token: expected 200, got {status}: {body}"
    );
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting a token: expected a nextToken, got {body}"))
        .to_string();

    for (case, mut request) in [
        (
            "another resource type",
            json!({"resourceType": RESOURCE_TYPE_EBS_VOLUME}),
        ),
        (
            "a scope narrowing the records",
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT},
            }),
        ),
    ] {
        request["nextToken"] = json!(token);
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused with 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // The selection the token WAS minted for resumes the walk it stopped.
    let mut request = selection.clone();
    request["maxResults"] = json!(1);
    request["nextToken"] = json!(token);
    let (status, body) = call(&router, operation, request).await;
    assert_eq!(
        status, 200,
        "the selection the token was minted for: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body["recommendationPreferencesDetails"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the selection the token was minted for: expected the next record, got {body}"
    );
}

/// One optional member a request narrows a reader's selection with, and the spellings of that
/// member which name the SAME selection.
struct SelectionSpellings {
    /// The operation whose input carries the member.
    operation: &'static str,
    /// The member, spelled as the wire spells it.
    member: &'static str,
    /// The values that all narrow NOTHING: `null` for a request that leaves the member out,
    /// and every empty form the member's own modelled shape admits.
    forms: Vec<Value>,
}

/// A list member, with the two spellings of "this list names nothing": the member left out,
/// and the member sent as the empty list.
fn list_member(operation: &'static str, member: &'static str) -> SelectionSpellings {
    SelectionSpellings {
        operation,
        member,
        forms: vec![Value::Null, json!([])],
    }
}

/// Every optional member the paginated readers of this service narrow their selection with,
/// crossed with the spellings of "this member narrows nothing".
///
/// The members are the ones this service's Smithy model gives those operations
/// (`smithy_query(query="operation",operation="GetEC2InstanceRecommendations,GetIdleRecommendations")`),
/// and the empty forms are the ones their target shapes admit: a list (`Filters`,
/// `EBSFilters`, `ResourceArns`, `JobIds`, `AccountIds`, …) is empty as `[]`, and a structure
/// whose every member is optional (`RecommendationPreferences`, `OrderBy`, `Scope`) is empty
/// as `{}` — and, where it holds a list of its own, as that list sent empty, or as a member
/// that constrains nothing without the member it qualifies (an `OrderBy$order` with no
/// `dimension` to sort on).
fn selection_spellings() -> Vec<SelectionSpellings> {
    let no_preferences = |operation| SelectionSpellings {
        operation,
        member: "recommendationPreferences",
        forms: vec![
            Value::Null,
            json!({}),
            json!({"cpuVendorArchitectures": []}),
        ],
    };
    vec![
        list_member("DescribeRecommendationExportJobs", "jobIds"),
        list_member("DescribeRecommendationExportJobs", "filters"),
        list_member("GetAutoScalingGroupRecommendations", "accountIds"),
        list_member("GetAutoScalingGroupRecommendations", "autoScalingGroupArns"),
        list_member("GetAutoScalingGroupRecommendations", "filters"),
        no_preferences("GetAutoScalingGroupRecommendations"),
        list_member("GetEBSVolumeRecommendations", "volumeArns"),
        list_member("GetEBSVolumeRecommendations", "filters"),
        list_member("GetEC2InstanceRecommendations", "accountIds"),
        list_member("GetEC2InstanceRecommendations", "instanceArns"),
        list_member("GetEC2InstanceRecommendations", "filters"),
        no_preferences("GetEC2InstanceRecommendations"),
        list_member("GetECSServiceRecommendations", "serviceArns"),
        list_member("GetECSServiceRecommendations", "filters"),
        list_member("GetEnrollmentStatusesForOrganization", "filters"),
        list_member("GetIdleRecommendations", "resourceArns"),
        list_member("GetIdleRecommendations", "filters"),
        SelectionSpellings {
            operation: "GetIdleRecommendations",
            member: "orderBy",
            forms: vec![Value::Null, json!({}), json!({"order": ORDER_DESC})],
        },
        list_member("GetLambdaFunctionRecommendations", "functionArns"),
        list_member("GetLambdaFunctionRecommendations", "filters"),
        list_member("GetLicenseRecommendations", "resourceArns"),
        list_member("GetLicenseRecommendations", "filters"),
        list_member("GetRDSDatabaseRecommendations", "resourceArns"),
        list_member("GetRDSDatabaseRecommendations", "filters"),
        no_preferences("GetRDSDatabaseRecommendations"),
        SelectionSpellings {
            operation: "GetRecommendationPreferences",
            member: "scope",
            forms: vec![Value::Null, json!({})],
        },
    ]
}

/// `reader`'s request with `member` spelled as `form` — left out entirely when the form is
/// `null` — plus the page size and the `nextToken` this call carries.
fn spelled_request(
    reader: &PagedReader,
    member: &str,
    form: &Value,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = reader.request.clone();
    if !form.is_null() {
        request[member] = form.clone();
    }
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    request
}

/// TWO SPELLINGS OF ONE SELECTION ARE ONE WALK: a reader resumes a `nextToken` minted for the
/// selection the request names, however that selection was spelled.
///
/// A selecting member a request leaves out and the empty value it can send instead name the
/// same result set — every reader of this service reads such a member defaulted — so the two
/// requests are one selection and the token either of them mints names a boundary of the
/// other's walk. Asserted over a table of every optional member each paginated reader selects
/// by, crossed with the empty forms that member's own modelled shape admits: every spelling
/// reports the same sequence, and every spelling's token resumes the walk of every other at
/// the same item — never refused as a token of some other selection, and never answered with
/// a page of one.
#[tokio::test]
async fn two_spellings_of_one_selection_resume_each_others_walk() {
    let (router, _engine) = paged_emulator().await;
    let readers = readers();

    for row in selection_spellings() {
        let reader = readers
            .iter()
            .find(|reader| reader.operation == row.operation)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the row must name a reader of the table",
                    row.operation
                )
            });

        // What each spelling selects, and the token a one-item page of its walk hands out.
        let mut spelled: Vec<(String, Vec<String>, String)> = Vec::new();
        for form in &row.forms {
            let case = format!("{} with {} sent as {form}", row.operation, row.member);
            let request = spelled_request(reader, row.member, form, None, None);
            let (status, body) = call(&router, row.operation, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from {request}, got {status}: {body}"
            );
            let sequence = page_of(&body, reader);
            assert!(
                sequence.len() >= 2,
                "case {case:?}: this fixture must hold at least two items for a page boundary to \
                 be observable, got {sequence:?}"
            );

            let request = spelled_request(reader, row.member, form, Some(1), None);
            let (status, body) = call(&router, row.operation, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from {request}, got {status}: {body}"
            );
            let token = body["nextToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a one-item page of a longer sequence must hand out a \
                         nextToken, got {body}"
                    )
                })
                .to_string();
            spelled.push((case, sequence, token));
        }

        // Every spelling selects the same sequence: they are ONE selection, not several.
        let (first_case, first_sequence, _) = &spelled[0];
        for (case, sequence, _) in &spelled[1..] {
            assert_eq!(
                sequence, first_sequence,
                "case {case:?}: expected the sequence {first_case:?} selects, \
                 {first_sequence:?}, got {sequence:?}"
            );
        }
        let expected = first_sequence[1..2].to_vec();

        // So each spelling's token resumes the walk of every spelling, at the item after the
        // page that minted it.
        for (minted_by, _, token) in &spelled {
            for form in &row.forms {
                let case = format!(
                    "{} with {} sent as {form}, resuming the token {minted_by:?} minted",
                    row.operation, row.member
                );
                let request = spelled_request(reader, row.member, form, Some(1), Some(token));
                let (status, body) = call(&router, row.operation, request.clone()).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {request} to resume the walk with 200, got \
                     {status}: {body}"
                );
                let page = page_of(&body, reader);
                assert_eq!(
                    page, expected,
                    "case {case:?}: expected the page {expected:?}, got {page:?}"
                );
            }
        }
    }
}

/// The readers whose selection the request's REGION scopes. Compute Optimizer is regional, so
/// each of these answers for the endpoint the request was signed for: the export jobs created
/// at it, or the resources whose ARNs it reports and matches a requested ARN against. A reader
/// absent from this list reports records no Region scopes — the organization's member accounts
/// (`GetEnrollmentStatusesForOrganization`) and the account's preference records
/// (`GetRecommendationPreferences`) — so its sequence, and so its page boundaries, are the
/// same at every endpoint.
const REGION_SCOPED_READERS: &[&str] = &[
    "DescribeRecommendationExportJobs",
    "GetAutoScalingGroupRecommendations",
    "GetEBSVolumeRecommendations",
    "GetEC2InstanceRecommendations",
    "GetECSServiceRecommendations",
    "GetIdleRecommendations",
    "GetLambdaFunctionRecommendations",
    "GetLicenseRecommendations",
    "GetRDSDatabaseRecommendations",
];

/// Regional endpoints other than the one the fixture's own calls address: a Region that reports
/// the same region-free analysis under its OWN ARNs, and one nothing was created at at all. A
/// token minted at the fixture's endpoint names a position in NEITHER of their walks.
const OTHER_REGIONS: &[&str] = &["eu-west-1", "ap-southeast-2"];

/// A `nextToken` resumes the walk of the REGIONAL ENDPOINT that minted it and of no other.
///
/// Compute Optimizer is regional: for every reader whose selection the request's Region scopes,
/// the sequence a token names a position in is that endpoint's, so the token is refused at
/// another endpoint rather than applied to its — different — sequence, which would skip items
/// or answer an empty final page a client reads as "the walk is finished". Asserted over the
/// whole table crossed with two other endpoints, with the two readers no Region scopes as the
/// control rows: their records are the same at every endpoint, so their tokens still resume
/// there.
#[tokio::test]
async fn a_next_token_resumes_only_the_regional_endpoint_that_minted_it() {
    let (router, _engine) = paged_emulator().await;

    for reader in readers() {
        let region_scoped = REGION_SCOPED_READERS.contains(&reader.operation);
        let whole = page_of(&read(&router, &reader, "the unpaged read", None, None).await, &reader);
        assert!(
            whole.len() >= 2,
            "case {:?}: this fixture must hold at least two items for a token to resume at one, \
             got {whole:?}",
            reader.operation
        );
        let body = read(&router, &reader, "minting a token", Some(1), None).await;
        let token = body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a one-item page of a longer sequence must hand out a nextToken, \
                     got {body}",
                    reader.operation
                )
            })
            .to_string();

        for other_region in OTHER_REGIONS {
            let label = format!(
                "{} resuming at {other_region} a token {REGION} minted",
                reader.operation
            );
            let mut request = reader.request.clone();
            request["maxResults"] = json!(1);
            request["nextToken"] = json!(token);
            let (status, body) =
                call_in_region(&router, other_region, reader.operation, request.clone()).await;
            if region_scoped {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected {request} to be refused with 400, got {status}: \
                     {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValueException",
                    "case {label:?}: expected InvalidParameterValueException, got {body}"
                );
                assert!(
                    body[reader.items].is_null(),
                    "case {label:?}: a refused read reports no page at all, got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a reader no Region scopes resumes its own token at every \
                     endpoint, got {status}: {body}"
                );
                let page = page_of(&body, &reader);
                assert_eq!(
                    page,
                    whole[1..2].to_vec(),
                    "case {label:?}: expected the page {:?}, got {page:?}",
                    &whole[1..2]
                );
            }
        }

        // The control for every row: at the endpoint that minted it, the same token still
        // resumes the walk it stopped.
        let label = format!(
            "{} resuming at {REGION}, where it was minted",
            reader.operation
        );
        let page = page_of(
            &read(&router, &reader, &label, Some(1), Some(&token)).await,
            &reader,
        );
        assert_eq!(
            page,
            whole[1..2].to_vec(),
            "case {label:?}: expected the page {:?}, got {page:?}",
            &whole[1..2]
        );
    }
}

/// One page of `reader`'s response, as the identities of the items it carries.
fn page_of(body: &Value, reader: &PagedReader) -> Vec<String> {
    body[reader.items]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a list of {}, got {body}",
                reader.operation, reader.items
            )
        })
        .iter()
        .map(|item| {
            let mut found = item;
            for member in reader.identity {
                found = &found[*member];
            }
            found
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected every item to carry {:?}, got {item}",
                        reader.operation, reader.identity
                    )
                })
                .to_string()
        })
        .collect()
}

/// Read one page from `reader`, asserting the call was accepted.
async fn read(
    router: &MultiServiceRouter,
    reader: &PagedReader,
    case: &str,
    max_results: Option<usize>,
    token: Option<&str>,
) -> Value {
    let mut request = reader.request.clone();
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let (status, body) = call(router, reader.operation, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// An emulator every reader in [`readers`] has at least two items to report from: the
/// account is enrolled, the organization holds member accounts, preference records are
/// written at four distinct scopes, export jobs exist for three resource types, two analysed
/// databases are recorded, the account's own Lambda functions are held with the utilization
/// published for them, and two Amazon ECS services have the analysis inputs a recommendation
/// is computed from.
async fn paged_emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_analysed_services_and_engine();
    register_analysed_functions(&router, &engine).await;
    opt_in(&router).await;
    // The analysis an enrollment starts is the gate every recommendation reader of this
    // service applies, so the fixture makes it due by moving the stored enrollment instant
    // back ([`make_analysis_due`]) rather than by waiting for the clock to reach it.
    make_analysis_due(&engine);
    record_organization(&engine);
    record_analysed_databases(&engine);
    write_preference_records(&router).await;
    create_bucket(&router, BUCKET).await;
    create_export_jobs(&router).await;
    analyse_ecs_services(&router, &engine).await;
    (router, engine)
}

/// Record member accounts for the organization, the way AWS Organizations would: no operation
/// of THIS API creates one, and `GetEnrollmentStatusesForOrganization` reports one row per
/// member account the store holds.
fn record_organization(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for account_id in ["200000000001", "200000000002", "200000000003"] {
        guard
            .organization_members
            .insert(MemberAccountEnrollment::new(account_id));
    }
}

/// Record two analysed Amazon Aurora or RDS databases into the findings catalog, the way
/// Compute Optimizer's own analysis of an observed workload would: a page size is only
/// observable over a sequence of more than one.
fn record_analysed_databases(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for identifier in ["db:ledger-primary", "db:ledger-replica"] {
        guard.analysed_resources.push(AnalysedResource {
            account_id: ::engine::emulator_account_id().to_string(),
            resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
            // The catalog records a resource's REGION-FREE identity; the ARN reported for it
            // names the region of the endpoint that reports it.
            arn_service: "rds".to_string(),
            arn_resource: identifier.to_string(),
            // An `RDSInstanceFinding` value with nothing to change, so the row needs no
            // recommendation option of its own.
            finding: "Optimized".to_string(),
            finding_reason_codes: Vec::new(),
            current_performance_risk: "VeryLow".to_string(),
            current_monthly_cost: 120.0,
            recommended_monthly_cost: 120.0,
            inferred_workload_types: Vec::new(),
            rds_database: None,
        });
    }
}

/// Write one preference record at each of four distinct scopes, which is what
/// `GetRecommendationPreferences` reports one row per. The value written is the one the
/// store already presents, so no other reader's recommendation changes.
async fn write_preference_records(router: &MultiServiceRouter) {
    let scopes = [
        json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT}),
        json!({"name": SCOPE_ORGANIZATION, "value": "ALL_ACCOUNTS"}),
        json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-0ab100000000000a1")}),
        json!({"name": SCOPE_RESOURCE_ARN, "value": instance_arn("i-0ab100000000000a2")}),
    ];
    for scope in scopes {
        put_preference(
            router,
            &json!({
                "resourceType": "Ec2Instance",
                "scope": scope,
                "enhancedInfrastructureMetrics": "Inactive",
            }),
        )
        .await;
    }
}

/// Create one export job per resource type, which is what `DescribeRecommendationExportJobs`
/// reports. The resource types differ because only one job per resource type may be in
/// progress in a Region at a time.
async fn create_export_jobs(router: &MultiServiceRouter) {
    for operation in [
        "ExportIdleRecommendations",
        "ExportEC2InstanceRecommendations",
        "ExportEBSVolumeRecommendations",
    ] {
        let (status, body) = call(
            router,
            operation,
            json!({"s3DestinationConfig": {"bucket": BUCKET}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the export to be accepted, got {status}: {body}"
        );
    }
}

/// Give two Amazon ECS services the inputs a recommendation is computed from: the service the
/// ECS store holds, and the `AWS/ECS` utilization published for it.
async fn analyse_ecs_services(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) {
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
        - 3_600.0;
    ecs_ok(router, "CreateCluster", json!({"clusterName": CLUSTER})).await;
    for service in ["alpha", "bravo"] {
        let family = format!("{service}-family");
        ecs_ok(
            router,
            "RegisterTaskDefinition",
            json!({
                "family": family,
                "networkMode": "awsvpc",
                "requiresCompatibilities": ["FARGATE"],
                "cpu": "1024",
                "memory": "2048",
                "containerDefinitions": [{
                    "name": "web",
                    "image": "public.ecr.aws/nginx/nginx:latest",
                    "cpu": 128,
                    "memory": 256,
                    "memoryReservation": 192,
                }],
            }),
        )
        .await;
        ecs_ok(
            router,
            "CreateService",
            json!({
                "cluster": CLUSTER,
                "serviceName": service,
                "taskDefinition": family,
                "desiredCount": 1,
                "launchType": "FARGATE",
                // The revision above is in the `awsvpc` network mode, whose tasks each get
                // their own elastic network interface, so the launch says which subnets and
                // security groups that interface takes.
                "networkConfiguration": {
                    "awsvpcConfiguration": {
                        "subnets": ["subnet-0123456789abcdef0"],
                        "securityGroups": ["sg-0123456789abcdef0"],
                    }
                },
            }),
        )
        .await;
        // `AWS/ECS` is a namespace AWS reserves for the service that owns the metric, so the
        // utilization analysed here is seeded the way ECS writes it.
        for metric_name in ["CPUUtilization", "MemoryUtilization"] {
            seed_cloudwatch_series(
                engine,
                "AWS/ECS",
                metric_name,
                &[("ClusterName", CLUSTER), ("ServiceName", service)],
                Some("Percent"),
                observed_at,
                &[20.0, 30.0],
            );
        }
    }
}

/// One Amazon ECS call that must be accepted.
async fn ecs_ok(router: &MultiServiceRouter, operation: &str, body: Value) {
    let (status, parsed) = call_ecs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: expected 200, got {status}: {parsed}"
    );
}
