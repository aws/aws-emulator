// SPDX-License-Identifier: CC0-1.0

//! `ListDistributionTenants`' `AssociationFilter` as a MATCH PREDICATE, end to end over
//! HTTP.
//!
//! The rule under test: each member of the filter is an unconstrained string the model asks
//! nothing of beyond naming "the distribution ID to filter by" and "the ID of the connection
//! group to filter by"
//! (`smithy_query(query="shape",shape="DistributionTenantAssociationFilter")`), so the value
//! narrows the listing to the tenants whose OWN record names that resource and is never a
//! precondition on the resource existing: a member naming a resource this account holds
//! none of matches no tenant, which is an ordinary empty page — HTTP 200 with an empty
//! `DistributionTenantList` and no `NextMarker` — exactly as a member naming a real resource
//! no tenant is associated with does.
//!
//! The rows differ along everything that rule generalises over: which member carries the
//! value (each one alone — exactly one of them is what the listing admits), whether the
//! named resource is absent or stored but associated with no tenant, the FORM of an absent
//! value (this service's own id shape, another resource's id shape, free text, a value far
//! longer than any id), and the page size the request asks for. Two positive rows name the
//! resources the fixture's tenant IS associated with, so an empty page is only ever the
//! filter matching nothing, and a second case pins that the refusals this listing really has
//! — a filter that does not name exactly one association, a `MaxItems` naming no page and a
//! `Marker` this service never handed out — are untouched by that.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the request helpers
//! are [`crate::distribution_tenants_list`]'s, and the fixture is created through
//! [`crate::distribution_tenant_lifecycle`]'s and [`crate::connection_groups`]'s helpers, so
//! every assertion travels the same path a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_TENANT_ID_PREFIX, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::distribution_tenants_list::{list_body, list_tenants, summaries, summary_ids};
use crate::response_headers_policy_lifecycle::{assert_absent, text};

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// What the fixture stored: the distribution and the connection group its one tenant is
/// associated with, a second distribution and a second connection group nothing is
/// associated with, and the tenant's id.
#[derive(Debug, Clone)]
struct Fixture {
    /// The multi-tenant distribution the tenant was created under.
    distribution: String,
    /// A second multi-tenant distribution, stored and holding no tenant at all.
    tenantless_distribution: String,
    /// The connection group the tenant is routed through.
    group: String,
    /// A second connection group, freshly created and routing no tenant at all.
    tenantless_group: String,
    /// The one tenant the fixture created.
    tenant: String,
}

/// Create one connection group named `name` over the wire and answer its minted id.
async fn connection_group(router: &MultiServiceRouter, name: &str) -> String {
    let body = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Enabled>true</Enabled></CreateConnectionGroupRequest>"
    );
    let answer = create_connection_group(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected HTTP 201 from CreateConnectionGroup, got {} with {}",
        answer.status, answer.raw
    );
    text(
        answer.root(),
        "Id",
        name,
        "the created connection group",
        &answer.raw,
    )
}

/// Stand the fixture up: two multi-tenant distributions, two connection groups, and ONE
/// tenant, associated with the first of each.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let distribution = multi_tenant_distribution(router, "ref-tenant-filter-match").await;
    let tenantless_distribution =
        multi_tenant_distribution(router, "ref-tenant-filter-no-tenant").await;
    let group = connection_group(router, "tenant-filter-group-routed").await;
    let tenantless_group = connection_group(router, "tenant-filter-group-empty").await;

    let slug = "filter-match-one";
    let body = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{slug}</Name>\
         <Domains><member><Domain>{slug}.example.com</Domain></member></Domains>\
         <ConnectionGroupId>{}</ConnectionGroupId>\
         <Parameters><member><Name>tenantName</Name><Value>{slug}</Value></member></Parameters>\
         {}</CreateDistributionTenantRequest>",
        escape(&distribution),
        escape(&group),
        managed_certificate_request()
    );
    let answer = create_distribution_tenant(router, &body).await;
    assert_eq!(
        answer.status, 201,
        "case {slug}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
        answer.status, answer.raw
    );
    let tenant = text(answer.root(), "Id", slug, "the created tenant", &answer.raw);

    Fixture {
        distribution,
        tenantless_distribution,
        group,
        tenantless_group,
        tenant,
    }
}

// ---------------------------------------------------------------------------
// The values the rows name
// ---------------------------------------------------------------------------

/// A distribution id in the shape CloudFront writes one in, which nothing here creates.
const ABSENT_DISTRIBUTION: &str = "E2QWRUHEXAMPLE1";

/// A connection group id in the shape CloudFront writes one in, which nothing here creates.
const ABSENT_GROUP: &str = "cg_ZZZZZZZZZZZZZ";

/// One row of the table: the two members the filter carries, and the tenant ids the listing
/// must report for them.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    distribution_id: Option<String>,
    connection_group_id: Option<String>,
    expected: Vec<String>,
}

impl Row {
    /// The request document this row sends under `max_items`.
    fn body(&self, max_items: Option<i32>) -> String {
        list_body(
            Some((
                self.distribution_id.as_deref(),
                self.connection_group_id.as_deref(),
            )),
            None,
            max_items,
        )
    }
}

/// A row naming one `DistributionId` and nothing else.
fn by_distribution(case: &'static str, id: &str, expected: Vec<String>) -> Row {
    Row {
        case,
        distribution_id: Some(id.to_string()),
        connection_group_id: None,
        expected,
    }
}

/// A row naming one `ConnectionGroupId` and nothing else.
fn by_group(case: &'static str, id: &str, expected: Vec<String>) -> Row {
    Row {
        case,
        distribution_id: None,
        connection_group_id: Some(id.to_string()),
        expected,
    }
}

/// A row naming both members at once — a filter the listing does not admit, since exactly
/// one association is what it filters by.
fn by_both(case: &'static str, distribution: &str, group: &str) -> Row {
    Row {
        case,
        distribution_id: Some(distribution.to_string()),
        connection_group_id: Some(group.to_string()),
        expected: Vec::new(),
    }
}

/// Every row the cases below are asserted over: the empty-page rows first, then the
/// positive controls that prove the filter still matches what the store holds.
fn rows(fixture: &Fixture) -> Vec<Row> {
    // A value far longer than any identifier this service mints, to span the form a filter
    // value can take.
    let overlong = "cg_".to_string() + &"Z".repeat(300);
    vec![
        by_group(
            "a connection group id this account holds none of",
            ABSENT_GROUP,
            Vec::new(),
        ),
        by_group(
            "a stored connection group no tenant is routed through",
            &fixture.tenantless_group,
            Vec::new(),
        ),
        by_group(
            "a connection group value of no id shape at all",
            "not a connection group !!",
            Vec::new(),
        ),
        by_group(
            "a connection group value carrying another resource's id",
            &fixture.distribution,
            Vec::new(),
        ),
        by_group(
            "a connection group value far longer than any id",
            &overlong,
            Vec::new(),
        ),
        by_distribution(
            "a distribution id this account holds none of",
            ABSENT_DISTRIBUTION,
            Vec::new(),
        ),
        by_distribution(
            "a stored distribution holding no tenant",
            &fixture.tenantless_distribution,
            Vec::new(),
        ),
        by_distribution(
            "a distribution value of no id shape at all",
            "not a distribution !!",
            Vec::new(),
        ),
        by_distribution(
            "a distribution value carrying another resource's id",
            &fixture.group,
            Vec::new(),
        ),
        by_distribution(
            "the distribution the tenant was created under",
            &fixture.distribution,
            vec![fixture.tenant.clone()],
        ),
        by_group(
            "the connection group the tenant is routed through",
            &fixture.group,
            vec![fixture.tenant.clone()],
        ),
    ]
}

/// Every filter the listing does NOT admit: it filters by exactly one association, so a
/// request naming neither member, or both at once, names no single association to filter by.
fn unadmitted_filters(fixture: &Fixture) -> Vec<Row> {
    vec![
        Row {
            case: "a filter naming neither member",
            distribution_id: None,
            connection_group_id: None,
            expected: Vec::new(),
        },
        by_both(
            "both members naming resources this account holds none of",
            ABSENT_DISTRIBUTION,
            ABSENT_GROUP,
        ),
        by_both(
            "a stored distribution beside an absent connection group",
            &fixture.distribution,
            ABSENT_GROUP,
        ),
        by_both(
            "an absent distribution beside the stored connection group",
            ABSENT_DISTRIBUTION,
            &fixture.group,
        ),
        by_both(
            "both resources the tenant's record names",
            &fixture.distribution,
            &fixture.group,
        ),
    ]
}

/// Assert one row's listing: the success status the model states, the tenants the row
/// expects, and a `NextMarker` only when a page was really cut (no row here cuts one).
fn assert_row(row: &Row, answer: &Answer, page: &str) {
    assert_eq!(
        answer.status, 200,
        "case {} ({page}): expected HTTP 200, got {} with {}",
        row.case, answer.status, answer.raw
    );
    let got = summary_ids(answer);
    assert_eq!(
        got, row.expected, "case {} ({page}): expected the tenants whose record names those resources ({:?}), got {got:?} with {}",
        row.case, row.expected, answer.raw
    );
    assert_absent(
        answer.root(),
        "NextMarker",
        row.case,
        "a listing with nothing left to resume from",
    );
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every filter value is a match predicate: one naming a resource this account holds none
/// of serves the SAME ordinary empty page as one naming a stored resource no tenant is
/// associated with, and one naming the tenant's own resources serves that tenant.
#[tokio::test]
async fn a_filter_value_matching_no_tenant_serves_an_empty_page() {
    let router = emulator();
    let fixture = fixture(&router).await;

    let rows = rows(&fixture);
    for row in &rows {
        let answer = list_tenants(&router, &row.body(None)).await;
        assert_row(row, &answer, "no page size");
    }

    // The empty page is an ordinary page, so the page size the request asks for changes
    // nothing about it: one past each end of what this listing serves, and the smallest and
    // the largest size it serves.
    for (page, max_items) in [
        ("the smallest page size", 1),
        ("a page size past the stored set", 25),
        ("this service's page ceiling", MAX_ITEMS),
    ] {
        for row in &rows {
            let answer = list_tenants(&router, &row.body(Some(max_items))).await;
            assert_row(row, &answer, page);
        }
    }

    // A listing reports; it stores nothing. After every call above, the account still holds
    // exactly the one tenant the fixture created.
    let answer = list_tenants(
        &router,
        &list_body(Some((Some(&fixture.distribution), None)), None, None),
    )
    .await;
    assert_eq!(
        summary_ids(&answer),
        vec![fixture.tenant.clone()],
        "case after every filtered listing: expected the one stored tenant, got {}",
        answer.raw
    );
}

/// The listing filters by exactly ONE association when it is given a filter at all, so a
/// filter element that names neither member, or both at once, is refused with the modelled
/// `InvalidArgument` (400) and the sentence that states the rule.
#[tokio::test]
async fn a_filter_that_names_no_single_association_is_refused() {
    let router = emulator();
    let fixture = fixture(&router).await;

    let mut cases: Vec<(&str, String)> = Vec::new();
    for row in unadmitted_filters(&fixture) {
        cases.push((row.case, row.body(None)));
    }
    for (case, body) in cases {
        let answer = list_tenants(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some("Exactly one association filter must be set."),
            "case {case}: expected the rule's own sentence, got {}",
            answer.raw
        );
        assert!(
            summaries(&answer).is_empty(),
            "case {case}: a refusal carries no listing, got {}",
            answer.raw
        );
    }
}

/// The refusals this listing really has are untouched by that: a `MaxItems` that names no
/// page, and a `Marker` this service never handed out, are refused even when the filter
/// beside them matches nothing.
#[tokio::test]
async fn the_page_and_marker_refusals_survive_a_filter_that_matches_nothing() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let unresolvable = Some((None, Some(ABSENT_GROUP)));

    for (case, max_items) in [("negative", -1), ("far negative", i32::MIN)] {
        let answer = list_tenants(&router, &list_body(unresolvable, None, Some(max_items))).await;
        assert_eq!(
            answer.status, 400,
            "case MaxItems {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case MaxItems {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A marker that is no distribution tenant id at all was never a resume point this
    // service handed out.
    let answer = list_tenants(
        &router,
        &list_body(unresolvable, Some("next-please"), None),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case a marker this service never handed out: expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidArgument",
        "case a marker this service never handed out: expected InvalidArgument, got {}",
        answer.raw
    );

    // A marker in the shape this service mints, naming no stored tenant: the entity is not
    // there, whatever the filter beside it names.
    let absent = format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", "Z".repeat(27));
    assert_ne!(
        fixture.tenant, absent,
        "case an absent marker: the fixture must not hold {absent}"
    );
    let answer = list_tenants(&router, &list_body(unresolvable, Some(&absent), None)).await;
    assert_eq!(
        answer.status, 404,
        "case an absent marker: expected HTTP 404, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "EntityNotFound",
        "case an absent marker: expected EntityNotFound, got {}",
        answer.raw
    );
    assert!(
        summaries(&answer).is_empty(),
        "case an absent marker: a refusal carries no listing, got {}",
        answer.raw
    );
}
