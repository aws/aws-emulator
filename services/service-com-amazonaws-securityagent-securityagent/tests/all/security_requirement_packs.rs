// SPDX-License-Identifier: CC0-1.0

//! The security requirement pack LISTING path of the SecurityAgent emulator, end to
//! end over the wire: `ListSecurityRequirementPacks` over the records
//! `CreateSecurityRequirementPack` stored (the create/read round trip itself lives in
//! `security_requirement_pack.rs`).
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (source: this crate's `OPERATIONS.md` "Wire protocol" section). So
//! nothing here can pass against a handler that answers a canned body: a value must
//! come back out of the shared store through `ListSecurityRequirementPacks` before a
//! case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call:
//! which members of a created pack a listed summary reports, which packs a filter
//! selects (the table is the cross product of the modelled `ManagementType` and
//! `SecurityRequirementPackStatus` catalogs plus "no member at all"), where a page
//! resumes at every page size, and what an input the service cannot act on answers.
//!
//! The router, the request builder, the error-code decoding and the `member` reader
//! are `application_lifecycle`'s `pub(crate)` harness — there is one harness for this
//! test target, not one per suite; only the `ok_with_status` wrapper (this slice's
//! create answers the model's 201) and the cases below are new.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::ops::types::{
    MANAGEMENT_TYPE_VALUES, SECURITY_REQUIREMENT_PACK_STATUS_VALUES,
};
use service_com_amazonaws_securityagent_securityagent::state::{
    AWS_MANAGED_PACKS, MANAGEMENT_TYPE_AWS_MANAGED, MANAGEMENT_TYPE_CUSTOMER_MANAGED,
    MAX_PAGE_SIZE, MAX_SECURITY_REQUIREMENT_PACKS, PACK_STATUS_ENABLED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member};

/// `call`, requiring `expected` as the HTTP status and naming the case when it is not.
///
/// `application_lifecycle::ok` requires 200, and `CreateSecurityRequirementPack`
/// answers the model's `"http": {"code": 201}`, so the expected status is a parameter
/// here.
async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected: u16,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected {operation} to answer {expected}, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `CreateSecurityRequirementPack`, returning the whole 201 payload.
///
/// The model gives this operation `"http": {"code": 201}`
/// (`smithy_query(query="operation",operation="CreateSecurityRequirementPack")`), so
/// the created-status itself is part of the contract each case asserts.
async fn create_pack(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    ok(router, "CreateSecurityRequirementPack", body, 201, case).await
}

/// `CreateSecurityRequirementPack`, returning the minted `packId`.
async fn create_pack_id(router: &MultiServiceRouter, body: &Value, case: &str) -> String {
    let payload = create_pack(router, body, case).await;
    member(&payload, "packId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateSecurityRequirementPack returned no packId: {payload}")
    })
}

/// One page of `ListSecurityRequirementPacks`: its summaries and its cursor.
struct Page {
    summaries: Vec<Value>,
    next_token: Option<String>,
}

/// `ListSecurityRequirementPacks` with `filter`, `maxResults` and `nextToken` as given.
async fn list_page(
    router: &MultiServiceRouter,
    filter: Option<&Value>,
    page_size: Option<i32>,
    token: Option<&str>,
    case: &str,
) -> Page {
    let mut request = json!({});
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    if let Some(size) = page_size {
        request["maxResults"] = json!(size);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    let payload = ok(
        router,
        "ListSecurityRequirementPacks",
        &request,
        200,
        case,
    )
    .await;
    let summaries = payload
        .get("securityRequirementPackSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: ListSecurityRequirementPacks answered no \
                 securityRequirementPackSummaries list: {payload}"
            )
        })
        .clone();
    Page {
        summaries,
        next_token: member(&payload, "nextToken"),
    }
}

/// Every summary `ListSecurityRequirementPacks` reports for `filter`, paging to the end
/// with `page_size` (or the service's default page size when `None`).
///
/// Asserts the pagination invariants on the way: no page is larger than it may be, a
/// page that carries a cursor is never empty, and the walk terminates.
async fn list_all(
    router: &MultiServiceRouter,
    filter: Option<&Value>,
    page_size: Option<i32>,
    case: &str,
) -> Vec<Value> {
    let ceiling = page_size
        .map(|size| (size.min(MAX_PAGE_SIZE)) as usize)
        .unwrap_or(MAX_PAGE_SIZE as usize);
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = list_page(router, filter, page_size, token.as_deref(), case).await;
        assert!(
            page.summaries.len() <= ceiling,
            "case {case:?}: a page held {} summaries, more than the {ceiling} it may",
            page.summaries.len()
        );
        if page.next_token.is_some() {
            assert!(
                !page.summaries.is_empty(),
                "case {case:?}: a page that carries a nextToken must not be empty"
            );
        }
        summaries.extend(page.summaries);
        token = page.next_token;
        if token.is_none() {
            break;
        }
        assert!(
            summaries.len() <= MAX_SECURITY_REQUIREMENT_PACKS,
            "case {case:?}: pagination did not terminate"
        );
    }
    summaries
}

/// The `packId` of each summary, in the order they were reported.
fn pack_ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            member(summary, "packId")
                .unwrap_or_else(|| panic!("a summary carries no packId: {summary}"))
        })
        .collect()
}

/// Only the summaries of packs a caller created.
///
/// A fresh account already holds the AWS managed catalog (`AWS_MANAGED_PACKS`), which
/// every unfiltered listing reports beside the created packs, so a case about what a
/// create put in the store looks at the customer managed ones.
fn customer_managed(summaries: &[Value]) -> Vec<Value> {
    summaries
        .iter()
        .filter(|summary| {
            member(summary, "managementType").as_deref()
                == Some(MANAGEMENT_TYPE_CUSTOMER_MANAGED)
        })
        .cloned()
        .collect()
}

/// The `packId`s of the seeded AWS managed catalog, in the order the store holds them
/// (its keys, sorted) — which is the order every listing walks.
fn seeded_pack_ids() -> Vec<String> {
    let mut ids: Vec<String> = AWS_MANAGED_PACKS
        .iter()
        .map(|entry| entry.pack_id.to_string())
        .collect();
    ids.sort();
    ids
}

/// A modelled `date-time` timestamp as epoch seconds.
///
/// `SecurityRequirementPackSummary$createdAt` and `$updatedAt` carry
/// `timestampFormat: date-time`, which restJson1 renders as an ISO-8601 string; a
/// number is accepted as epoch seconds so the case asserts the VALUE rather than one
/// rendering of it.
fn timestamp_secs(summary: &Value, name: &str, case: &str) -> f64 {
    let value = summary
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: a summary carries no {name}: {summary}"));
    match value {
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| {
            panic!("case {case:?}: {name} {text:?} is not an RFC 3339 date-time: {e}")
        })
        .as_secs_f64(),
        Value::Number(number) => number.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {name} {number} is not a finite epoch timestamp")
        }),
        other => panic!("case {case:?}: {name} is not a modelled timestamp: {other}"),
    }
}

/// The input rows the round-trip case spans: they differ along which optional members
/// the request carries, the form of each value, the status asked for and the size of
/// the tag map — the dimensions `CreateSecurityRequirementPack`'s input generalises
/// over (`smithy_query(query="operation",operation="CreateSecurityRequirementPack")`:
/// `description`, `status`, `kmsKeyId` and `tags`, all optional).
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("the required name alone", json!({"name": "baseline"})),
        (
            "a name and a description",
            json!({"name": "described", "description": "the controls for the payments tier"}),
        ),
        (
            // The status catalog is `SecurityRequirementPackStatus`
            // (`smithy_query(query="shape",shape="SecurityRequirementPackStatus")`),
            // and the model documents ENABLED as the default when it is absent.
            "the status named explicitly as ENABLED",
            json!({"name": "explicitly-enabled", "status": "ENABLED"}),
        ),
        (
            "the status named as DISABLED",
            json!({"name": "born-disabled", "status": "DISABLED"}),
        ),
        (
            // `KmsKeyId` is an unconstrained string, so the rows carry a bare key id,
            // a key ARN and an alias — the forms a KMS key is named by.
            "a bare KMS key id",
            json!({"name": "kms-bare", "kmsKeyId": "12345678-1234-1234-1234-123456789012"}),
        ),
        (
            "a KMS key ARN",
            json!({
                "name": "kms-arn",
                "kmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012",
            }),
        ),
        (
            "a KMS alias",
            json!({"name": "kms-alias", "kmsKeyId": "alias/packs"}),
        ),
        (
            "several tags",
            json!({
                "name": "tagged",
                "tags": {"env": "prod", "team": "appsec", "cost-center": "4711"},
            }),
        ),
        ("an empty tag map", json!({"name": "untagged", "tags": {}})),
        (
            "every optional member at once",
            json!({
                "name": "everything",
                "description": "all of it",
                "status": "DISABLED",
                "kmsKeyId": "alias/everything",
                "tags": {"env": "test"},
            }),
        ),
        (
            // A name is an unconstrained string, so a long one, a Unicode one and one
            // carrying surrounding whitespace are all names the service stores
            // verbatim.
            "a long name",
            json!({"name": "p".repeat(256)}),
        ),
        (
            "a Unicode name",
            json!({"name": "pack-caf\u{e9}-\u{5b89}\u{5168}"}),
        ),
        (
            "a name with inner and surrounding whitespace",
            json!({"name": "  spaced out  "}),
        ),
    ]
}

/// RULE: a listed summary reports each member from the record
/// `CreateSecurityRequirementPack` persisted — the supplied `name` and `description`
/// verbatim, the status the caller asked for or the model's documented `ENABLED`
/// default, and the service's own `packId`, `managementType` and timestamps. Nothing
/// is echoed from the List request and nothing is a placeholder.
#[tokio::test]
async fn created_packs_are_reported_by_the_list() {
    let router = emulator();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut expected: BTreeMap<String, (&str, Value)> = BTreeMap::new();

    for (case, request) in create_rows() {
        let created = create_pack(&router, &request, case).await;
        let pack_id = member(&created, "packId")
            .unwrap_or_else(|| panic!("case {case:?}: no packId was minted: {created}"));
        assert!(
            !pack_id.is_empty(),
            "case {case:?}: expected a non-empty packId, got {pack_id:?}"
        );
        assert!(
            seen_ids.insert(pack_id.clone()),
            "case {case:?}: packId {pack_id:?} was minted twice"
        );

        let wanted_status = member(&request, "status")
            .unwrap_or_else(|| PACK_STATUS_ENABLED.to_string());
        assert_eq!(
            member(&created, "status").as_deref(),
            Some(wanted_status.as_str()),
            "case {case:?}: expected the created status {wanted_status:?}, got {created}"
        );
        assert_eq!(
            member(&created, "kmsKeyId"),
            member(&request, "kmsKeyId"),
            "case {case:?}: expected the supplied kmsKeyId {:?}, got {:?}",
            member(&request, "kmsKeyId"),
            member(&created, "kmsKeyId")
        );
        expected.insert(pack_id, (case, request));
    }

    let listed = list_all(&router, None, None, "every created pack").await;
    let summaries = customer_managed(&listed);
    assert_eq!(
        listed.len(),
        expected.len() + AWS_MANAGED_PACKS.len(),
        "case every created pack: expected the {} created packs beside the {} seeded \
         AWS managed ones, got {}",
        expected.len(),
        AWS_MANAGED_PACKS.len(),
        listed.len()
    );
    assert_eq!(
        summaries.len(),
        expected.len(),
        "case every created pack: expected {} summaries, got {}",
        expected.len(),
        summaries.len()
    );

    for summary in &summaries {
        let pack_id = member(summary, "packId")
            .unwrap_or_else(|| panic!("a summary carries no packId: {summary}"));
        let (case, request) = expected
            .get(&pack_id)
            .unwrap_or_else(|| panic!("the list reported an unknown packId {pack_id:?}"));
        assert_eq!(
            member(summary, "name"),
            member(request, "name"),
            "case {case:?}: expected the stored name {:?}, got {:?}",
            member(request, "name"),
            member(summary, "name")
        );
        assert_eq!(
            member(summary, "description"),
            member(request, "description"),
            "case {case:?}: expected the stored description {:?}, got {:?}",
            member(request, "description"),
            member(summary, "description")
        );
        let wanted_status = member(request, "status")
            .unwrap_or_else(|| PACK_STATUS_ENABLED.to_string());
        assert_eq!(
            member(summary, "status").as_deref(),
            Some(wanted_status.as_str()),
            "case {case:?}: expected the stored status {wanted_status:?}, got {summary}"
        );
        // A pack a caller created through this API is customer managed, and only an
        // AWS managed pack carries a vendor.
        assert_eq!(
            member(summary, "managementType").as_deref(),
            Some(MANAGEMENT_TYPE_CUSTOMER_MANAGED),
            "case {case:?}: expected a {MANAGEMENT_TYPE_CUSTOMER_MANAGED} pack, got {summary}"
        );
        assert!(
            summary.get("vendorName").is_none(),
            "case {case:?}: a customer managed pack has no vendorName: {summary}"
        );
        // Both timestamps are required, and a pack nothing has updated since it was
        // created carries one instant in both.
        let created_at = timestamp_secs(summary, "createdAt", case);
        let updated_at = timestamp_secs(summary, "updatedAt", case);
        assert!(
            created_at > 0.0,
            "case {case:?}: expected a real createdAt, got {created_at}"
        );
        assert!(
            (updated_at - created_at).abs() < f64::EPSILON,
            "case {case:?}: expected updatedAt to equal createdAt {created_at}, got {updated_at}"
        );
    }

    // The list is a read: a second call answers exactly the same summaries.
    let again = list_all(&router, None, None, "a repeated list").await;
    assert_eq!(
        again, listed,
        "case a repeated list: two reads of the pack store disagreed"
    );
}

/// RULE: the identity is minted per call, not derived from the request — two
/// `CreateSecurityRequirementPack` calls with the same name are two distinct packs, and
/// the list reports both.
#[tokio::test]
async fn packs_sharing_one_name_are_both_listed() {
    let router = emulator();
    let request = json!({"name": "the same name", "description": "the same description"});

    let mut ids = Vec::new();
    for attempt in 0..4 {
        ids.push(create_pack_id(&router, &request, &format!("attempt {attempt}")).await);
    }
    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case identical requests: expected {} distinct packIds, got {ids:?}",
        ids.len()
    );

    let summaries = list_all(&router, None, None, "identical requests").await;
    let listed: BTreeSet<String> = pack_ids(&customer_managed(&summaries))
        .into_iter()
        .collect();
    assert_eq!(
        listed, unique.into_iter().cloned().collect::<BTreeSet<_>>(),
        "case identical requests: the list reported {listed:?} for the created {ids:?}"
    );
    for summary in &customer_managed(&summaries) {
        assert_eq!(
            member(summary, "name"),
            member(&request, "name"),
            "case identical requests: expected each pack to keep its name: {summary}"
        );
    }
}

/// RULE: a `filter` member selects exactly the packs whose corresponding field equals
/// it, an absent member selects every value of that field, and the members compose.
///
/// The table is the cross product of "no member" with each value of the modelled
/// catalogs — `MANAGEMENT_TYPE_VALUES` and `SECURITY_REQUIREMENT_PACK_STATUS_VALUES`,
/// the generated renderings of `ManagementType` and `SecurityRequirementPackStatus` —
/// so `AWS_MANAGED`, which this API cannot create, is a row too.
#[tokio::test]
async fn the_filter_selects_exactly_the_packs_it_names() {
    let router = emulator();

    // A mix the filter has something to choose from: three enabled and two disabled.
    let mut created: Vec<(String, &str)> = Vec::new();
    for (index, status) in ["ENABLED", "DISABLED", "ENABLED", "ENABLED", "DISABLED"]
        .into_iter()
        .enumerate()
    {
        let case = format!("pack {index} ({status})");
        let id = create_pack_id(
            &router,
            &json!({"name": format!("pack-{index}"), "status": status}),
            &case,
        )
        .await;
        created.push((id, status));
    }

    let management_types: Vec<Option<&str>> = std::iter::once(None)
        .chain(MANAGEMENT_TYPE_VALUES.iter().map(|value| Some(*value)))
        .collect();
    let statuses: Vec<Option<&str>> = std::iter::once(None)
        .chain(
            SECURITY_REQUIREMENT_PACK_STATUS_VALUES
                .iter()
                .map(|value| Some(*value)),
        )
        .collect();

    for management_type in &management_types {
        for status in &statuses {
            let case = format!("filter managementType={management_type:?} status={status:?}");
            let mut filter = json!({});
            if let Some(management_type) = management_type {
                filter["managementType"] = json!(management_type);
            }
            if let Some(status) = status {
                filter["status"] = json!(status);
            }

            // Every pack this API creates is CUSTOMER_MANAGED and every seeded one is
            // AWS_MANAGED, so a filter naming a management type selects from exactly one
            // of the two groups. Both groups are keyed by a pack id of the one class
            // form, so the store's keys — and so the listing — walk them in one sorted
            // order, whichever group a key belongs to.
            let mut wanted: Vec<String> = created
                .iter()
                .filter(|(_, pack_status)| {
                    management_type
                        .is_none_or(|wanted| wanted == MANAGEMENT_TYPE_CUSTOMER_MANAGED)
                        && status.is_none_or(|wanted| wanted == *pack_status)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let mut seeded: Vec<String> = AWS_MANAGED_PACKS
                .iter()
                .filter(|entry| {
                    management_type.is_none_or(|wanted| wanted == MANAGEMENT_TYPE_AWS_MANAGED)
                        && status.is_none_or(|wanted| wanted == entry.status)
                })
                .map(|entry| entry.pack_id.to_string())
                .collect();
            wanted.append(&mut seeded);
            wanted.sort();

            // The whole result in one page, and the same result paged one at a time:
            // the cursor must respect the filter too.
            for page_size in [None, Some(1)] {
                let got = pack_ids(&list_all(&router, Some(&filter), page_size, &case).await);
                assert_eq!(
                    got, wanted,
                    "case {case:?} at page size {page_size:?}: expected {wanted:?}, got {got:?}"
                );
            }
        }
    }
}

/// RULE: a page holds at most the requested number of summaries, the cursor names the
/// record the next page resumes AT, and paging reports every pack exactly once in one
/// stable order whatever the page size is.
#[tokio::test]
async fn pagination_reports_every_pack_exactly_once() {
    let router = emulator();
    let mut ids = Vec::new();
    for index in 0..7 {
        let case = format!("pack {index}");
        ids.push(create_pack_id(&router, &json!({"name": format!("pack-{index}")}), &case).await);
    }
    // Every stored pack: the ones created here, then the seeded AWS managed catalog.
    let stored: Vec<String> = ids.iter().cloned().chain(seeded_pack_ids()).collect();

    // The order one full page reports is the order every other page size must agree
    // with.
    let order = pack_ids(&list_all(&router, None, None, "the default page size").await);
    assert_eq!(
        order.len(),
        stored.len(),
        "case the default page size: expected {} packs, got {order:?}",
        stored.len()
    );
    assert_eq!(
        order.iter().collect::<BTreeSet<_>>(),
        stored.iter().collect::<BTreeSet<_>>(),
        "case the default page size: expected the stored packs {stored:?}, got {order:?}"
    );

    // Each row is a page size: below the record count, exactly it, above it, and past
    // the service's own ceiling (which is clamped, never refused).
    for page_size in [1, 2, 3, 6, 7, 8, 100, MAX_PAGE_SIZE + 1, i32::MAX] {
        let case = format!("page size {page_size}");
        let walked = pack_ids(&list_all(&router, None, Some(page_size), &case).await);
        assert_eq!(
            walked, order,
            "case {case:?}: expected the packs in the order {order:?}, got {walked:?}"
        );
    }

    // The first page of a one-at-a-time walk carries a cursor and exactly one summary;
    // resuming from it starts at the NEXT pack, never repeating the first.
    let first = list_page(&router, None, Some(1), None, "the first page").await;
    assert_eq!(
        first.summaries.len(),
        1,
        "case the first page: expected one summary, got {:?}",
        first.summaries
    );
    let token = first
        .next_token
        .expect("case the first page: more packs remain, so a nextToken is due");
    let second = list_page(&router, None, Some(1), Some(&token), "the second page").await;
    assert_eq!(
        pack_ids(&second.summaries),
        vec![order[1].clone()],
        "case the second page: expected the pack after {:?}, got {:?}",
        order[0],
        second.summaries
    );

    // The last page carries no cursor.
    let size = i32::try_from(stored.len()).expect("the stored pack count fits an i32");
    let whole = list_page(&router, None, Some(size), None, "one whole page").await;
    assert_eq!(
        whole.summaries.len(),
        stored.len(),
        "case one whole page: expected every pack in one page, got {:?}",
        whole.summaries
    );
    assert!(
        whole.next_token.is_none(),
        "case one whole page: nothing remains, so no nextToken is due: {:?}",
        whole.next_token
    );
}

/// RULE: an input the list cannot act on is refused with HTTP 400
/// `ValidationException` — a page size below one, a `nextToken` this list did not mint
/// (including one another list did), and a filter value outside the modelled enum.
#[tokio::test]
async fn inputs_the_service_cannot_act_on_are_refused() {
    let router = emulator();
    create_pack_id(&router, &json!({"name": "only-pack"}), "the stored pack").await;

    // A cursor from ANOTHER list of this same service: it is well formed, and still not
    // a resume point for this one.
    for index in 0..2 {
        ok(
            &router,
            "CreateApplication",
            &json!({}),
            200,
            &format!("application {index}"),
        )
        .await;
    }
    let applications = ok(
        &router,
        "ListApplications",
        &json!({"maxResults": 1}),
        200,
        "an application page",
    )
    .await;
    let foreign_token = member(&applications, "nextToken")
        .expect("two applications exist, so ListApplications owes a nextToken");

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a page size of zero",
            "ListSecurityRequirementPacks",
            json!({"maxResults": 0}),
        ),
        (
            "a nextToken this service never minted",
            "ListSecurityRequirementPacks",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a nextToken with this list's prefix and a corrupt body",
            "ListSecurityRequirementPacks",
            json!({"nextToken": "pack-v1:zzzz"}),
        ),
        (
            "another list's nextToken",
            "ListSecurityRequirementPacks",
            json!({"nextToken": foreign_token}),
        ),
        (
            "a managementType outside the modelled enum",
            "ListSecurityRequirementPacks",
            json!({"filter": {"managementType": "CUSTOMER"}}),
        ),
        (
            "a negative page size",
            "ListSecurityRequirementPacks",
            json!({"maxResults": -5}),
        ),
        (
            "a status in the wrong case",
            "ListSecurityRequirementPacks",
            json!({"filter": {"status": "enabled"}}),
        ),
    ];

    for (case, operation, request) in rows {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected {operation} to refuse with 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?} with {}",
            answer.error_code, answer.body
        );
    }

    // Every refusal above left the store exactly as it was.
    let summaries = customer_managed(&list_all(&router, None, None, "after the refusals").await);
    assert_eq!(
        summaries.len(),
        1,
        "case after the refusals: expected the one stored pack, got {summaries:?}"
    );
}

/// RULE: a fresh account holds no pack of its own, so a listing filtered to customer
/// managed packs is an empty page — not an error and not a cursor. (What a fresh account
/// DOES hold, the seeded AWS managed catalog, is
/// `security_requirement_pack_lifecycle.rs`'s subject.)
#[tokio::test]
async fn a_fresh_account_lists_no_customer_managed_pack() {
    let router = emulator();
    for (case, filter) in [
        (
            "a management type filter",
            json!({"managementType": MANAGEMENT_TYPE_CUSTOMER_MANAGED}),
        ),
        (
            "a management type and a status filter",
            json!({"managementType": MANAGEMENT_TYPE_CUSTOMER_MANAGED, "status": "ENABLED"}),
        ),
    ] {
        let page = list_page(&router, Some(&filter), None, None, case).await;
        assert!(
            page.summaries.is_empty(),
            "case {case:?}: expected no summaries, got {:?}",
            page.summaries
        );
        assert!(
            page.next_token.is_none(),
            "case {case:?}: an empty page owes no nextToken, got {:?}",
            page.next_token
        );
    }
}
