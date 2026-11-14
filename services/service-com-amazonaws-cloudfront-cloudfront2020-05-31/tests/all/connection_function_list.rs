// SPDX-License-Identifier: CC0-1.0

//! `ListConnectionFunctions`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "Lists connection functions": the listing answers from the STORED connection functions,
//!   each listed ONCE and in the order the store holds them (by the id the create minted),
//!   and every item is the SAME projection of the stored stage record a
//!   `DescribeConnectionFunction` of that id and stage answers with;
//! * "The connection function's stage": a request that names no stage lists every connection
//!   function under the stage one is created in, and `Stage=LIVE` lists exactly the ones a
//!   publish has copied into that stage — so a connection function appears there when, and
//!   only when, it was published;
//! * the page contract: `MaxItems` is the page size, `Marker` resumes after the id the
//!   previous page ended at ("The response includes items in the list that occur after the
//!   marker"), and `NextMarker` arrives exactly when matching connection functions remain;
//! * a `MaxItems` no page size can be taken from, and a `Marker` that is not a resume point
//!   this service handed out, are refused with the modelled `InvalidArgument`, while a
//!   `Stage` outside the value set the modelled enum declares is refused as a request whose
//!   schema this API cannot read (`MalformedXML`);
//!
//! The rows span what those rules generalise over: the whole connection function create table
//! (every name character class, both runtimes, each code size and a configuration that
//! associates a key value store), which of them are published, every page size from one item
//! to past the whole collection, both stages, and resume points above and below everything
//! the store holds.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_function_lifecycle::{
    create_connection_function, create_table, describe_connection_function, key_value_store_arn,
    member, Create,
};
use crate::connection_function_publish::{awaited_publish, publish_connection_function};
use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The status a stage that has never been published reports, and the one a published stage no
/// distribution names reports.
const STATUS_UNPUBLISHED: &str = "UNPUBLISHED";
const STATUS_UNASSOCIATED: &str = "UNASSOCIATED";

/// Which rows of the create table are published, by index — so the `LIVE` listing has both
/// something to list and something to leave out, and the published rows are not contiguous.
const PUBLISHED_ROWS: &[usize] = &[0, 2, 4];

// ---------------------------------------------------------------------------
// Calling the listing
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/connection-functions` with the request document `body` — the route the
/// model binds this operation to.
///
/// `pub(crate)` so the suite that checks a DELETE leaves the listing behind drives the
/// listing through this one call path rather than a second copy of it.
pub(crate) async fn list_connection_functions(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/connection-functions"),
        Some(body),
        None,
    )
    .await
}

/// A `ListConnectionFunctionsRequest` document naming whichever of the three input members the
/// case under test names, and leaving every other one out entirely.
pub(crate) fn list_body(
    max_items: Option<i32>,
    marker: Option<&str>,
    stage: Option<&str>,
) -> String {
    let mut out = format!(
        "<ListConnectionFunctionsRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(marker) = marker {
        out.push_str(&format!("<Marker>{marker}</Marker>"));
    }
    if let Some(max_items) = max_items {
        out.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    if let Some(stage) = stage {
        out.push_str(&format!("<Stage>{stage}</Stage>"));
    }
    out.push_str("</ListConnectionFunctionsRequest>");
    out
}

/// The `ConnectionFunctionSummary` elements one listing answered with.
///
/// `ConnectionFunctionSummaryList`'s member carries `xmlName` `ConnectionFunctionSummary`
/// (`smithy_query(query="shape",shape="ConnectionFunctionSummaryList")`), so each entry travels
/// in an element of that name inside the `ConnectionFunctions` wrapper.
pub(crate) fn summaries(answer: &Answer) -> Vec<&Node> {
    match answer.root().at("ConnectionFunctions") {
        Some(wrapper) => wrapper.children_named("ConnectionFunctionSummary"),
        None => Vec::new(),
    }
}

/// One page of the listing, as the document reports it.
struct Page {
    /// `ConnectionFunctionSummary$Id` of each item, in document order.
    ids: Vec<String>,
    /// `ConnectionFunctionSummary$Name` of each item, in document order.
    names: Vec<String>,
    /// `ConnectionFunctionSummary$Stage` of each item, in document order.
    stages: Vec<String>,
    /// `ConnectionFunctionSummary$Status` of each item, in document order.
    statuses: Vec<String>,
    /// `NextMarker`, or `None` when the element is absent.
    next_marker: Option<String>,
}

/// Read the page `answer` carries, asserting it is a listing at all.
fn page(answer: &Answer, case: &str) -> Page {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from the listing, got {} with {}",
        answer.status, answer.raw
    );
    let items = summaries(answer);
    let text = |path: &str| -> Vec<String> {
        items
            .iter()
            .map(|summary| member(summary, path, case, &answer.raw))
            .collect()
    };
    Page {
        ids: text("Id"),
        names: text("Name"),
        stages: text("Stage"),
        statuses: text("Status"),
        next_marker: answer.root().text_at("NextMarker").map(str::to_string),
    }
}

// ---------------------------------------------------------------------------
// Seeding the store
// ---------------------------------------------------------------------------

/// What one create minted, beside the request it was created from.
struct Seeded {
    create: Create,
    id: String,
    /// Whether [`PUBLISHED_ROWS`] named this row, so it has a `LIVE` stage.
    published: bool,
}

/// Create every row of the connection function create table, publishing the rows
/// [`PUBLISHED_ROWS`] names, and answer what each create minted.
async fn seed(router: &MultiServiceRouter, store_name: &str) -> Vec<Seeded> {
    let kvs_arn = key_value_store_arn(router, store_name).await;
    let mut seeded = Vec::new();
    for (index, create) in create_table(&kvs_arn).into_iter().enumerate() {
        let case = create.case;
        let answer = create_connection_function(router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            answer.status, answer.raw
        );
        let id = member(answer.root(), "Id", case, &answer.raw);
        let published = PUBLISHED_ROWS.contains(&index);
        if published {
            let etag = answer.header("etag").to_string();
            let answered = publish_connection_function(router, &id, &etag).await;
            assert_eq!(
                answered.status, 200,
                "case {case}: expected HTTP 200 from the publish, got {} with {}",
                answered.status, answered.raw
            );
            // A publish is asynchronous, so the seed waits for the status the CLOCK owes the
            // stage it wrote: what this suite asserts is the projection of a settled stage.
            awaited_publish(router, case, &id, STATUS_UNASSOCIATED).await;
        }
        seeded.push(Seeded {
            create,
            id,
            published,
        });
    }
    seeded
}

/// The ids of the seeded connection functions a listing of `stage` must serve, in the order it
/// must serve them: a connection function is held under the id its create minted, so the order
/// is the ids' own rather than the order the creates were made in.
fn expected_ids(seeded: &[Seeded], stage: &str) -> Vec<String> {
    let mut ids: Vec<String> = seeded
        .iter()
        .filter(|row| stage == STAGE_DEVELOPMENT || row.published)
        .map(|row| row.id.clone())
        .collect();
    ids.sort();
    ids
}

/// The seeded row the id `id` names.
fn row<'a>(seeded: &'a [Seeded], id: &str) -> &'a Seeded {
    seeded
        .iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("the listing reported the id {id}, which no create minted"))
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The listing answers from the stored connection functions, in id order, and the stage decides
/// WHICH record each item is projected from: every connection function under the stage one is
/// created in, and exactly the published ones under `LIVE`. Each item is the same projection the
/// describe of that id and stage answers with, member for member.
#[tokio::test]
async fn the_listing_reports_the_stored_connection_functions_of_the_stage_asked_for() {
    let router = emulator();
    let seeded = seed(&router, "connection-function-list-store").await;

    for (case, stage_member, stage, status) in [
        (
            "no stage at all",
            None,
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
        ),
        (
            "the stage a connection function is created in",
            Some(STAGE_DEVELOPMENT),
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
        ),
        (
            "the published stage",
            Some(STAGE_LIVE),
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
        ),
    ] {
        let answer = list_connection_functions(&router, &list_body(None, None, stage_member)).await;
        let page = page(&answer, case);
        let expected = expected_ids(&seeded, stage);
        assert_eq!(
            page.ids, expected,
            "case {case}: expected the stored connection functions in id order, got {:?}",
            page.ids
        );
        assert_eq!(
            page.names,
            expected
                .iter()
                .map(|id| row(&seeded, id).create.name.clone())
                .collect::<Vec<String>>(),
            "case {case}: expected the name each connection function was created under, got {:?}",
            page.names
        );
        assert_eq!(
            page.stages,
            vec![stage.to_string(); expected.len()],
            "case {case}: expected every item in the {stage} stage, got {:?}",
            page.stages
        );
        assert_eq!(
            page.statuses,
            vec![status.to_string(); expected.len()],
            "case {case}: expected every item to report Status {status}, got {:?}",
            page.statuses
        );
        assert_eq!(
            page.next_marker, None,
            "case {case}: a page that ends the collection carries no NextMarker, got {:?}",
            page.next_marker
        );

        // The listed item and the describe of the same id and stage are ONE projection of ONE
        // stored record, so every member of the summary agrees — including the configuration
        // and the timestamps the stage holds.
        for id in &expected {
            let described = describe_connection_function(&router, id, Some(stage)).await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected HTTP 200 describing {id} in {stage}, got {} with {}",
                described.status, described.raw
            );
            let listed = summaries(&answer);
            let listed = listed
                .iter()
                .find(|summary| summary.text_at("Id") == Some(id.as_str()))
                .unwrap_or_else(|| panic!("case {case}: the page lists {id}"));
            for path in [
                "Name",
                "Id",
                "ConnectionFunctionArn",
                "Status",
                "Stage",
                "CreatedTime",
                "LastModifiedTime",
                "ConnectionFunctionConfig/Comment",
                "ConnectionFunctionConfig/Runtime",
            ] {
                let in_page = member(listed, path, case, &answer.raw);
                let in_describe = member(described.root(), path, case, &described.raw);
                assert_eq!(
                    in_page, in_describe,
                    "case {case}: expected the listing's {path} for {id} to be the stored \
                     record's, got {in_page:?} against the describe's {in_describe:?}"
                );
            }
        }
    }
}

/// A connection function appears under `LIVE` when, and only when, a publish has copied it
/// there: each publish adds exactly its own connection function to that listing, and the stage
/// a connection function is created in is unaffected by it.
#[tokio::test]
async fn the_live_listing_reports_exactly_what_a_publish_promoted() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection-function-live-list-store").await;
    let table = create_table(&kvs_arn);
    let mut created: Vec<(Create, String, String)> = Vec::new();
    for create in table {
        let case = create.case;
        let answer = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            answer.status, answer.raw
        );
        let id = member(answer.root(), "Id", case, &answer.raw);
        let etag = answer.header("etag").to_string();
        created.push((create, id, etag));
    }
    let mut all_ids: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    all_ids.sort();

    // Before any publish the published stage lists nothing at all, while every stored
    // connection function is in the stage it was created in.
    let empty = list_connection_functions(&router, &list_body(None, None, Some(STAGE_LIVE))).await;
    let empty = page(&empty, "before any publish");
    assert!(
        empty.ids.is_empty(),
        "case before any publish: expected the published stage to list nothing, got {:?}",
        empty.ids
    );
    assert_eq!(
        empty.next_marker, None,
        "case before any publish: a page that ends the collection carries no NextMarker, got {:?}",
        empty.next_marker
    );

    let mut promoted: Vec<String> = Vec::new();
    for (create, id, etag) in &created {
        let case = create.case;
        let published = publish_connection_function(&router, id, etag).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 from the publish, got {} with {}",
            published.status, published.raw
        );
        promoted.push(id.clone());
        promoted.sort();

        let answer = list_connection_functions(&router, &list_body(None, None, Some(STAGE_LIVE)))
            .await;
        let live = page(&answer, case);
        assert_eq!(
            live.ids, promoted,
            "case {case}: expected exactly the published connection functions, got {:?}",
            live.ids
        );

        let answer =
            list_connection_functions(&router, &list_body(None, None, Some(STAGE_DEVELOPMENT)))
                .await;
        let development = page(&answer, case);
        assert_eq!(
            development.ids, all_ids,
            "case {case}: expected every stored connection function under {STAGE_DEVELOPMENT}, \
             got {:?}",
            development.ids
        );
    }
}

/// The page contract: `MaxItems` bounds the page, `Marker` resumes after the id the previous
/// page ended at, `NextMarker` arrives exactly while matching connection functions remain, and
/// walking the pages enumerates every one of them exactly once, in id order.
#[tokio::test]
async fn the_pages_enumerate_every_connection_function_exactly_once() {
    let router = emulator();
    let seeded = seed(&router, "connection-function-page-store").await;

    for (stage_case, stage_member, stage) in [
        ("no stage at all", None, STAGE_DEVELOPMENT),
        ("the published stage", Some(STAGE_LIVE), STAGE_LIVE),
    ] {
        let expected = expected_ids(&seeded, stage);
        // Every page size from one item up to one past the whole collection.
        for size in 1..=expected.len() + 1 {
            let case = format!("{stage_case}, MaxItems {size}");
            let mut walked: Vec<String> = Vec::new();
            let mut seen = BTreeSet::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;

            loop {
                let body = list_body(
                    Some(i32::try_from(size).expect("the page size fits an i32")),
                    marker.as_deref(),
                    stage_member,
                );
                let answer = list_connection_functions(&router, &body).await;
                let page = page(&answer, &case);
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case}: the paging walk does not terminate, got {pages} pages"
                );
                assert!(
                    page.ids.len() <= size,
                    "case {case}: expected at most {size} items on a page, got {:?}",
                    page.ids
                );
                for id in &page.ids {
                    assert!(
                        seen.insert(id.clone()),
                        "case {case}: the connection function {id} was listed twice"
                    );
                }
                walked.extend(page.ids.iter().cloned());

                let remaining = expected.len() > walked.len();
                match (&page.next_marker, remaining) {
                    (Some(next), true) => {
                        assert_eq!(
                            Some(next.as_str()),
                            page.ids.last().map(String::as_str),
                            "case {case}: expected NextMarker to name the id the page ended at, \
                             got {next:?}"
                        );
                        marker = Some(next.clone());
                    }
                    (None, false) => break,
                    (next, remaining) => panic!(
                        "case {case}: expected NextMarker present exactly while connection \
                         functions remain ({remaining} remaining), got {next:?}"
                    ),
                }
            }

            assert_eq!(
                walked, expected,
                "case {case}: expected the pages to enumerate every connection function in id \
                 order, got {walked:?}"
            );
        }
    }
}

/// A `MaxItems` no page size can be taken from is refused with the modelled `InvalidArgument`
/// and lists nothing; a size past the whole collection is a page that simply ends it.
#[tokio::test]
async fn a_max_items_no_page_size_can_be_taken_from_is_refused() {
    let router = emulator();
    let seeded = seed(&router, "connection-function-max-items-store").await;

    for (case, value) in [("a negative page size", -1), ("far negative", i32::MIN)] {
        let answer = list_connection_functions(&router, &list_body(Some(value), None, None)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for MaxItems={value}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, MAX_ITEMS_INVALID_MESSAGE,
            "case {case}: expected the constraint's own sentence, got {message:?}"
        );
    }

    // A page size past the whole collection, and one past this service's own page ceiling, each
    // end the listing in one page.
    for (case, size) in [
        ("a page size past the whole collection", MAX_ITEMS),
        ("a page size past the page ceiling", MAX_ITEMS + 1),
    ] {
        let answer = list_connection_functions(&router, &list_body(Some(size), None, None)).await;
        let page = page(&answer, case);
        assert_eq!(
            page.ids,
            expected_ids(&seeded, STAGE_DEVELOPMENT),
            "case {case}: expected every stored connection function, got {:?}",
            page.ids
        );
        assert_eq!(
            page.next_marker, None,
            "case {case}: a page that ends the collection carries no NextMarker, got {:?}",
            page.next_marker
        );
    }
}

/// A `Stage` outside the value set the modelled enum declares is no stage at all: the value
/// set is part of the schema this API publishes for the request, so the listing is refused as
/// a request that cannot be read, while a value the enum admits is served.
#[tokio::test]
async fn a_stage_outside_the_modelled_enum_is_refused_as_a_request_that_cannot_be_read() {
    let router = emulator();
    let seeded = seed(&router, "connection-function-stage-store").await;

    // The forms an inadmissible value takes, built from a value the enum DOES admit so the
    // rows come from the value set itself rather than from a literal.
    for (case, value) in [
        ("another word", "NOT_A_STAGE".to_string()),
        (
            "an admitted value in the wrong case",
            STAGE_DEVELOPMENT.to_lowercase(),
        ),
        (
            "an admitted value with trailing text",
            format!("{STAGE_LIVE}X"),
        ),
        ("the empty value", String::new()),
    ] {
        let answer =
            list_connection_functions(&router, &list_body(None, None, Some(&value))).await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({value:?}): expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "MalformedXML",
            "case {case} ({value:?}): expected MalformedXML, got {:?} in {}",
            answer.error_code(),
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert!(
            message.contains("stage"),
            "case {case} ({value:?}): expected the refusal to name the member, got {message:?}"
        );
        assert!(
            message.contains(&format!("[{STAGE_LIVE}, {STAGE_DEVELOPMENT}]")),
            "case {case} ({value:?}): expected the refusal to state the value set, got \
             {message:?}"
        );
    }

    // The control for the rule: each value the enum DOES admit is served, so every refusal
    // above is about the value rather than about the member being there at all.
    for stage in [STAGE_DEVELOPMENT, STAGE_LIVE] {
        let answer = list_connection_functions(&router, &list_body(None, None, Some(stage))).await;
        let page = page(&answer, stage);
        assert_eq!(
            page.ids,
            expected_ids(&seeded, stage),
            "case {stage}: expected the stored connection functions of the stage, got {:?}",
            page.ids
        );
    }
}

/// A `Marker` this service never handed out is refused with the modelled `InvalidArgument`,
/// while one in the shape of an id it mints is a resume point: the page it serves is exactly
/// the stored connection functions that occur after it.
#[tokio::test]
async fn a_marker_this_service_never_handed_out_is_refused() {
    let router = emulator();
    let seeded = seed(&router, "connection-function-marker-store").await;
    let all = expected_ids(&seeded, STAGE_DEVELOPMENT);

    // Every way a value can fail to be a connection function id this service mints: another
    // resource kind's id, the prefix alone, a body of the wrong length, a body outside the id
    // alphabet, and a name a connection function really answers to elsewhere — a name is not a
    // resume point.
    for (case, marker) in [
        ("a value that is no id at all", "not-a-marker".to_string()),
        ("a distribution id", "E1A2B3C4D5E6F7".to_string()),
        ("the prefix alone", "cf_".to_string()),
        (
            "a body that is too short",
            format!("cf_{}", "A".repeat(26)),
        ),
        (
            "a body that is too long",
            format!("cf_{}", "A".repeat(28)),
        ),
        (
            "a body outside the id alphabet",
            format!("cf_{}", "-".repeat(27)),
        ),
        (
            "the name a stored connection function answers to",
            seeded[0].create.name.clone(),
        ),
    ] {
        let answer =
            list_connection_functions(&router, &list_body(None, Some(&marker), None)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for Marker={marker}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A resume point in the shape of a minted id lists exactly what occurs after it, whether or
    // not the store still holds a connection function under it: the first id the store holds,
    // one past the last, and each id the store does hold.
    let mut resume_points = vec![
        ("below every stored id", format!("cf_{}", "0".repeat(27))),
        ("past every stored id", format!("cf_{}", "z".repeat(27))),
    ];
    resume_points.extend(all.iter().map(|id| ("a stored id", id.clone())));
    for (case, marker) in resume_points {
        let answer =
            list_connection_functions(&router, &list_body(None, Some(&marker), None)).await;
        let page = page(&answer, case);
        let expected: Vec<String> = all
            .iter()
            .filter(|id| id.as_str() > marker.as_str())
            .cloned()
            .collect();
        assert_eq!(
            page.ids, expected,
            "case {case}: expected the ids after the marker {marker}, got {:?}",
            page.ids
        );
        assert_eq!(
            page.next_marker, None,
            "case {case}: a page that ends the collection carries no NextMarker, got {:?}",
            page.next_marker
        );
    }
}
