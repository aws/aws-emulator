// SPDX-License-Identifier: CC0-1.0

//! Listing the account's distribution tenants, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * `ListDistributionTenants` reports the distribution tenants this account's store holds,
//!   each summary read from the record its create persisted, so a listing and a read of the
//!   same tenant can never disagree;
//! * `AssociationFilter` narrows the listing to the tenants ASSOCIATED with ONE resource —
//!   "the distribution ID to filter by" or "the ID of the connection group to filter by" —
//!   so the member it names narrows it to the tenants whose own record names that one
//!   resource, and exactly one of the two has to be named;
//! * the listing reports the tenants in ONE stable total order, cut to at most `MaxItems`
//!   entries beginning after the `Marker` id, and carries a `NextMarker` exactly when
//!   tenants remain — so walking the pages at ANY admissible page size enumerates them
//!   exactly once;
//! * a `MaxItems` that names no page, a `Marker` this service never handed out, and a filter
//!   that names neither or both of its members, are refused; a well-formed marker naming no
//!   stored tenant is an entity that does not exist; and a filter member carrying no value
//!   names no resource, so it is as good as absent.
//!
//! What a filter member naming a resource this account holds none of answers is the rule
//! [`crate::distribution_tenant_filter_matches_nothing`] is about.
//!
//! The rows differ along what the rules generalise over: which of the two multi-tenant
//! distributions a tenant was created under, which connection group routes it (two groups a
//! caller created and the account's default one, which a create that names none asks for),
//! how many tenants share one of those resources, which members the filter names (neither,
//! each alone, both together, and a pair that shares no tenant), and every page size from
//! one up past the stored set, plus this service's own page ceiling and one past it.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the tenants are
//! created through [`crate::distribution_tenant_lifecycle`]'s helpers and the connection
//! groups through [`crate::connection_groups`]'s, so these assertions travel the same path a
//! client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_TENANT_ID_PREFIX, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, escape, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::response_headers_policy_lifecycle::{assert_absent, text};

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distribution-tenants` with the request document `body`.
///
/// `pub(crate)` so a sibling suite asserting another of this listing's rules sends its
/// requests down this one wire path rather than growing a copy of it.
pub(crate) async fn list_tenants(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution-tenants"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Id}`.
async fn get_tenant(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        None,
        None,
    )
    .await
}

/// A `ListDistributionTenantsRequest` document naming whichever input members the case under
/// test names, in the order the shape declares them.
///
/// `filter` is `Some` when the request carries an `AssociationFilter` element at all — a
/// `Some((None, None))` sends the element with neither member inside it.
pub(crate) fn list_body(
    filter: Option<(Option<&str>, Option<&str>)>,
    marker: Option<&str>,
    max_items: Option<i32>,
) -> String {
    let mut out = format!(
        "<ListDistributionTenantsRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some((distribution_id, connection_group_id)) = filter {
        out.push_str("<AssociationFilter>");
        if let Some(distribution_id) = distribution_id {
            out.push_str(&format!(
                "<DistributionId>{}</DistributionId>",
                escape(distribution_id)
            ));
        }
        if let Some(connection_group_id) = connection_group_id {
            out.push_str(&format!(
                "<ConnectionGroupId>{}</ConnectionGroupId>",
                escape(connection_group_id)
            ));
        }
        out.push_str("</AssociationFilter>");
    }
    if let Some(marker) = marker {
        out.push_str(&format!("<Marker>{}</Marker>", escape(marker)));
    }
    if let Some(max_items) = max_items {
        out.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    out.push_str("</ListDistributionTenantsRequest>");
    out
}

/// The `DistributionTenantSummary` elements one listing answered with.
///
/// `DistributionTenantList`'s member carries `xmlName` `DistributionTenantSummary`
/// (`smithy_query(query="shape",shape="DistributionTenantList")`), so each entry travels in
/// an element of that name inside the `DistributionTenantList` wrapper.
pub(crate) fn summaries(answer: &Answer) -> Vec<&Node> {
    match answer.root().at("DistributionTenantList") {
        Some(wrapper) => wrapper.children_named("DistributionTenantSummary"),
        None => Vec::new(),
    }
}

/// The ids one listing reported, in the order it reported them.
pub(crate) fn summary_ids(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| {
            summary
                .text_at("Id")
                .expect("every summary carries an Id")
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The tenants under test
// ---------------------------------------------------------------------------

/// Which connection group routes one tenant: one of the two a caller created, or the
/// account's DEFAULT one — "if you don't specify a connection group, CloudFront uses the
/// default connection group", so a create that names none is routed through that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// The create names no `ConnectionGroupId` at all.
    Default,
    /// The create names the first group of [`GROUP_NAMES`].
    First,
    /// The create names the second group of [`GROUP_NAMES`].
    Second,
}

/// The two connection groups the fixture creates, so a tenant can be routed through a group
/// that is not the account's default one.
const GROUP_NAMES: [&str; 2] = ["tenant-list-group-one", "tenant-list-group-two"];

/// One distribution tenant the table creates.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// Names the tenant and its domain, both unique in the account.
    slug: &'static str,
    /// Which of the two multi-tenant distributions this tenant belongs to.
    distribution: usize,
    /// Which connection group routes it.
    group: Group,
}

/// The tenants every case below is asserted over, spanning both distributions, all three
/// connection groups, and several tenants sharing one of each.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the first distribution, the account default group",
            slug: "alpha-default-one",
            distribution: 0,
            group: Group::Default,
        },
        Row {
            case: "a second tenant of the same distribution and group",
            slug: "alpha-default-two",
            distribution: 0,
            group: Group::Default,
        },
        Row {
            case: "a third one, so a page is really cut",
            slug: "alpha-default-three",
            distribution: 0,
            group: Group::Default,
        },
        Row {
            case: "the first distribution, the first named group",
            slug: "alpha-first-one",
            distribution: 0,
            group: Group::First,
        },
        Row {
            case: "a second tenant sharing that distribution and group",
            slug: "alpha-first-two",
            distribution: 0,
            group: Group::First,
        },
        Row {
            case: "the first distribution, the second named group",
            slug: "alpha-second-one",
            distribution: 0,
            group: Group::Second,
        },
        Row {
            case: "the second distribution, the first named group",
            slug: "beta-first-one",
            distribution: 1,
            group: Group::First,
        },
        Row {
            case: "a second tenant of the second distribution in that group",
            slug: "beta-first-two",
            distribution: 1,
            group: Group::First,
        },
        Row {
            case: "the second distribution, the account default group",
            slug: "beta-default-one",
            distribution: 1,
            group: Group::Default,
        },
    ]
}

/// One tenant as the create that stored it left it: the row, its minted id, the resources
/// its record associates it with, and its version token.
#[derive(Debug, Clone)]
struct Created {
    row: Row,
    id: String,
    /// `DistributionTenant$DistributionId`, as the create's own answer reports it.
    distribution_id: String,
    /// `DistributionTenant$ConnectionGroupId`, as the create's own answer reports it — which
    /// is how the id of the account's automatic default group is learnt at all.
    connection_group_id: String,
    etag: String,
}

/// The whole fixture: the two multi-tenant distributions, the two named connection groups
/// and every tenant of [`rows`].
#[derive(Debug, Clone)]
struct Fixture {
    distributions: Vec<String>,
    named_groups: Vec<String>,
    created: Vec<Created>,
}

impl Fixture {
    /// The ids the table says a listing under `distribution` and `group` must report, in the
    /// order the service reports them (tenant id order).
    ///
    /// Computed from the RECORD each create persisted, which is what the rule is about: a
    /// tenant is reported when its own record names every resource the filter names.
    fn expected(&self, distribution: Option<&str>, group: Option<&str>) -> Vec<String> {
        let mut ids: Vec<String> = self
            .created
            .iter()
            .filter(|created| {
                distribution.is_none_or(|wanted| created.distribution_id == wanted)
                    && group.is_none_or(|wanted| created.connection_group_id == wanted)
            })
            .map(|created| created.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The connection group id one row's tenant is routed through, read off what the create
    /// persisted.
    fn group_id(&self, group: Group) -> String {
        match group {
            Group::First => self.named_groups[0].clone(),
            Group::Second => self.named_groups[1].clone(),
            Group::Default => self
                .created
                .iter()
                .find(|created| created.row.group == Group::Default)
                .expect("the table creates a tenant routed through the default group")
                .connection_group_id
                .clone(),
        }
    }
}

/// The `CreateDistributionTenantRequest` XML for `row` under `distribution_id`, naming
/// `connection_group_id` when the row is routed through a group a caller created.
fn tenant_xml(row: &Row, distribution_id: &str, connection_group_id: Option<&str>) -> String {
    let group = match connection_group_id {
        Some(id) => format!("<ConnectionGroupId>{}</ConnectionGroupId>", escape(id)),
        None => String::new(),
    };
    format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{}</Name>\
         <Domains><member><Domain>{}.example.com</Domain></member></Domains>\
         {}\
         <Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>\
         {}</CreateDistributionTenantRequest>",
        escape(distribution_id),
        row.slug,
        row.slug,
        group,
        row.slug,
        managed_certificate_request()
    )
}

/// Stand the whole fixture up over the wire: two multi-tenant distributions, two connection
/// groups, and every tenant of [`rows`].
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let mut distributions = Vec::new();
    for reference in ["ref-tenant-list-alpha", "ref-tenant-list-beta"] {
        distributions.push(multi_tenant_distribution(router, reference).await);
    }

    let mut named_groups = Vec::new();
    for name in GROUP_NAMES {
        let body = format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name></CreateConnectionGroupRequest>"
        );
        let answer = create_connection_group(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {name}: expected HTTP 201 from CreateConnectionGroup, got {} with {}",
            answer.status, answer.raw
        );
        named_groups.push(text(
            answer.root(),
            "Id",
            name,
            "the created connection group",
            &answer.raw,
        ));
    }

    let mut created = Vec::new();
    for row in rows() {
        let group = match row.group {
            Group::Default => None,
            Group::First => Some(named_groups[0].as_str()),
            Group::Second => Some(named_groups[1].as_str()),
        };
        let body = tenant_xml(&row, &distributions[row.distribution], group);
        let answer = create_distribution_tenant(router, &body).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
            row.case, answer.status, answer.raw
        );
        created.push(Created {
            row,
            id: text(answer.root(), "Id", row.case, "the created tenant", &answer.raw),
            distribution_id: text(
                answer.root(),
                "DistributionId",
                row.case,
                "the created tenant",
                &answer.raw,
            ),
            connection_group_id: text(
                answer.root(),
                "ConnectionGroupId",
                row.case,
                "the created tenant",
                &answer.raw,
            ),
            etag: answer.header("etag").to_string(),
        });
    }

    Fixture {
        distributions,
        named_groups,
        created,
    }
}

/// The `AssociationFilter` one case sends: whichever of its two members that case names.
///
/// A member left `None` is absent from the request document; a member carrying the empty
/// string travels as an element with no value, which names no resource.
#[derive(Debug, Clone, Default)]
struct Filter {
    distribution_id: Option<String>,
    connection_group_id: Option<String>,
}

impl Filter {
    /// The filter naming one distribution and nothing else.
    fn distribution(id: &str) -> Self {
        Self {
            distribution_id: Some(id.to_string()),
            connection_group_id: None,
        }
    }

    /// The filter naming one connection group and nothing else.
    fn connection_group(id: &str) -> Self {
        Self {
            distribution_id: None,
            connection_group_id: Some(id.to_string()),
        }
    }

    /// The filter naming both resources.
    fn both(distribution_id: &str, connection_group_id: &str) -> Self {
        Self {
            distribution_id: Some(distribution_id.to_string()),
            connection_group_id: Some(connection_group_id.to_string()),
        }
    }

    /// The two ids this filter names, as the expectation reads them.
    fn parts(&self) -> (Option<&str>, Option<&str>) {
        (
            self.distribution_id.as_deref(),
            self.connection_group_id.as_deref(),
        )
    }
}

/// The two ids a case's filter names, or none at all for a request carrying no filter.
fn parts_of(filter: Option<&Filter>) -> (Option<&str>, Option<&str>) {
    filter.map(Filter::parts).unwrap_or((None, None))
}

/// Every filter the cases below run: no filter element at all — the optional input member
/// absent, which narrows by nothing — and then each member alone, over each resource the
/// fixture stored, which is what a filter element that IS carried admits: exactly one
/// association to filter by.
fn filters(fixture: &Fixture) -> Vec<(String, Option<Filter>)> {
    let first_distribution = fixture.distributions[0].clone();
    let second_distribution = fixture.distributions[1].clone();
    let default_group = fixture.group_id(Group::Default);
    let first_group = fixture.group_id(Group::First);
    let second_group = fixture.group_id(Group::Second);
    vec![
        ("no filter element at all".to_string(), None),
        (
            "the first distribution".to_string(),
            Some(Filter::distribution(&first_distribution)),
        ),
        (
            "the second distribution".to_string(),
            Some(Filter::distribution(&second_distribution)),
        ),
        (
            "the account's default connection group".to_string(),
            Some(Filter::connection_group(&default_group)),
        ),
        (
            "a connection group several tenants share".to_string(),
            Some(Filter::connection_group(&first_group)),
        ),
        (
            "a connection group one tenant is routed through".to_string(),
            Some(Filter::connection_group(&second_group)),
        ),
    ]
}

/// The request one filter row sends, under `marker` and `max_items`.
fn body_for(filter: Option<&Filter>, marker: Option<&str>, max_items: Option<i32>) -> String {
    list_body(filter.map(Filter::parts), marker, max_items)
}

// ---------------------------------------------------------------------------
// The listing, and its filter
// ---------------------------------------------------------------------------

/// Each filter reports exactly the tenants whose own record names the resource it names, and
/// the two distributions between them account for every tenant the fixture stored.
#[tokio::test]
async fn each_filter_reports_the_tenants_whose_record_names_those_resources() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for (case, filter) in filters(&fixture) {
        let answer = list_tenants(&router, &body_for(filter.as_ref(), None, None)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let (distribution, group) = parts_of(filter.as_ref());
        let expected = fixture.expected(distribution, group);
        let got = summary_ids(&answer);
        assert_eq!(
            got, expected,
            "case {case}: expected the tenants associated with those resources, got {got:?} \
             with {}",
            answer.raw
        );
        // The whole stored set fits one page, so nothing remains to resume from.
        assert_absent(answer.root(), "NextMarker", &case, "the only page");
    }

    // Every tenant the fixture stored belongs to one of the two distributions, so the two
    // listings between them report the whole stored set: a listing narrows only by what the
    // filter names.
    let mut listed: Vec<String> = Vec::new();
    for distribution in &fixture.distributions {
        let filter = Filter::distribution(distribution);
        let answer = list_tenants(&router, &body_for(Some(&filter), None, None)).await;
        listed.extend(summary_ids(&answer));
    }
    listed.sort();
    let mut all: Vec<String> = fixture
        .created
        .iter()
        .map(|created| created.id.clone())
        .collect();
    all.sort();
    assert_eq!(
        listed, all,
        "case the two distributions together: expected every stored tenant, got {listed:?}"
    );
}

/// Each summary reports the same values a read of that one tenant reports — one record, two
/// readers — including the version token the create handed back, and carries no member the
/// summary shape does not declare.
#[tokio::test]
async fn each_summary_reports_what_a_read_of_the_same_tenant_reports() {
    let router = emulator();
    let fixture = fixture(&router).await;

    // The members a summary and the full document state under the same name. Read as
    // `Option`, so a member absent from one must be absent from the other too.
    let shared = [
        "Id",
        "DistributionId",
        "Name",
        "Arn",
        "ConnectionGroupId",
        "Status",
        "Enabled",
        "CreatedTime",
        "LastModifiedTime",
    ];

    // The listing filters by exactly one association, so the whole stored set is read
    // through one listing per distribution: every tenant belongs to one of the two.
    let mut listings: Vec<Answer> = Vec::new();
    for distribution in &fixture.distributions {
        let filter = Filter::distribution(distribution);
        let answer = list_tenants(&router, &body_for(Some(&filter), None, None)).await;
        assert_eq!(
            answer.status, 200,
            "case the listing of one distribution: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        listings.push(answer);
    }

    let mut seen = 0usize;
    for answer in &listings {
        for summary in summaries(answer) {
            seen += 1;
            let id = text(summary, "Id", "a summary", "the listing", &answer.raw);
            let created = fixture
                .created
                .iter()
                .find(|created| created.id == id)
                .unwrap_or_else(|| {
                    panic!("case a summary: the listing reported an unknown id {id}")
                });
            let case = created.row.case;
            let read = get_tenant(&router, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected HTTP 200 from GetDistributionTenant, got {} with {}",
                read.status, read.raw
            );
            for path in shared {
                assert_eq!(
                    summary.text_at(path),
                    read.root().text_at(path),
                    "case {case}: the summary and the read disagree about {path}: {} against {}",
                    answer.raw,
                    read.raw
                );
            }
            assert_eq!(
                domains_of(summary),
                domains_of(read.root()),
                "case {case}: the summary and the read disagree about Domains: {}",
                answer.raw
            );
            // `DistributionTenantSummary$ETag` is "the current version of the distribution
            // tenant": the token the create handed back, which the read reports as its
            // header.
            assert_eq!(
                summary.text_at("ETag"),
                Some(created.etag.as_str()),
                "case {case}: expected the tenant's current version in the summary, got {}",
                answer.raw
            );
            assert_eq!(
                read.header("etag"),
                created.etag,
                "case {case}: expected the read to report the same version, got {}",
                read.raw
            );
            // The summary shape declares no `Parameters`, so the parameter values a tenant
            // holds are reached through a read of the tenant itself.
            assert_absent(summary, "Parameters", case, "a summary");
            assert!(
                read.root().at("Parameters").is_some(),
                "case {case}: the read carries the tenant's parameters, got {}",
                read.raw
            );
        }
    }
    assert_eq!(
        seen,
        fixture.created.len(),
        "case every stored tenant: expected a summary for each of the {} tenants, saw {seen}",
        fixture.created.len()
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Walk every page of one filter's listing at `page_size` and answer the ids in the order
/// they were reported.
async fn walk_pages(
    router: &MultiServiceRouter,
    filter: Option<&Filter>,
    page_size: i32,
    case: &str,
) -> Vec<String> {
    let case = format!("{case} at page size {page_size}");
    let mut ids: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..64 {
        let body = body_for(filter, marker.as_deref(), Some(page_size));
        let answer = list_tenants(router, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let page = summary_ids(&answer);
        assert!(
            page.len() <= page_size as usize,
            "case {case}: a page holds at most {page_size} tenants, got {page:?}"
        );
        ids.extend(page.iter().cloned());
        match answer.root().text_at("NextMarker") {
            Some(next) => {
                assert_eq!(
                    Some(next.to_string()),
                    page.last().cloned(),
                    "case {case}: the NextMarker is the id the page ended at, got {}",
                    answer.raw
                );
                marker = Some(next.to_string());
            }
            // No `NextMarker`: this was the last page.
            None => return ids,
        }
    }
    panic!("case {case}: the page walk did not terminate");
}

/// The listing cuts a page at `MaxItems`, resumes after the `Marker` and hands back a
/// `NextMarker` exactly when tenants remain — so walking the pages at ANY admissible page
/// size enumerates the matching set exactly once, in one stable order.
#[tokio::test]
async fn the_listing_pages_and_resumes_from_the_marker() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for (case, filter) in filters(&fixture) {
        let (distribution, group) = parts_of(filter.as_ref());
        let all = fixture.expected(distribution, group);

        // One row per page size from the smallest admissible one to one past the stored
        // set's size, plus the largest size this service serves and one past it.
        for page_size in [1, 2, 3, 4, 8, 9, 10, MAX_ITEMS, MAX_ITEMS + 1] {
            let walked = walk_pages(&router, filter.as_ref(), page_size, &case).await;
            assert_eq!(
                walked, all,
                "case {case} at page size {page_size}: expected every matching tenant exactly \
                 once in id order, got {walked:?}"
            );
        }

        if let Some(last) = all.last() {
            // The marker the last page ended at resumes AFTER it: nothing is left to report.
            let answer =
                list_tenants(&router, &body_for(filter.as_ref(), Some(last), None)).await;
            assert_eq!(
                answer.status, 200,
                "case {case} resumed past the last match: expected HTTP 200, got {} with {}",
                answer.status, answer.raw
            );
            assert!(
                summaries(&answer).is_empty(),
                "case {case} resumed past the last match: expected nothing after the last one, \
                 got {}",
                answer.raw
            );
        }
    }

    // A marker that names a stored tenant the filter does NOT match is still a resume point:
    // it says where to start, not what to report.
    let filter = Filter::distribution(&fixture.distributions[1]);
    let matching = fixture.expected(Some(&fixture.distributions[1]), None);
    let unmatched = fixture
        .created
        .iter()
        .find(|created| created.distribution_id != fixture.distributions[1])
        .expect("the table stores tenants of the other distribution");
    let answer =
        list_tenants(&router, &body_for(Some(&filter), Some(&unmatched.id), None)).await;
    let expected: Vec<String> = matching
        .iter()
        .filter(|id| *id > &unmatched.id)
        .cloned()
        .collect();
    assert_eq!(
        summary_ids(&answer),
        expected,
        "case resumed from a tenant of another distribution: expected the matches after it, got {}",
        answer.raw
    );
}

// ---------------------------------------------------------------------------
// A filter that names no single association
// ---------------------------------------------------------------------------

/// A filter member carrying NO value names no resource, so it is as good as absent: the
/// listing admits exactly ONE named association, so a request whose only named member is
/// empty names none and is refused, while an empty member BESIDE a named one leaves exactly
/// one and is served.
#[tokio::test]
async fn a_filter_member_carrying_no_value_names_no_association() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for (case, filter) in [
        ("an empty distribution element", Filter::distribution("")),
        (
            "an empty connection group element",
            Filter::connection_group(""),
        ),
        ("both members empty", Filter::both("", "")),
        (
            "both members named",
            Filter::both(
                &fixture.distributions[0],
                &fixture.group_id(Group::First),
            ),
        ),
    ] {
        let answer = list_tenants(&router, &body_for(Some(&filter), None, None)).await;
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
        assert!(
            summaries(&answer).is_empty(),
            "case {case}: a refusal carries no listing, got {}",
            answer.raw
        );
    }

    // An empty member beside a named one: exactly one association is named, so the listing
    // is the named resource's own.
    for (case, filter) in [
        (
            "a named distribution beside an empty connection group element",
            Filter::both(&fixture.distributions[0], ""),
        ),
        (
            "a named connection group beside an empty distribution element",
            Filter::both("", &fixture.group_id(Group::First)),
        ),
    ] {
        let answer = list_tenants(&router, &body_for(Some(&filter), None, None)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let (distribution, group) = filter.parts();
        let expected = fixture.expected(
            distribution.filter(|id| !id.is_empty()),
            group.filter(|id| !id.is_empty()),
        );
        assert_eq!(
            summary_ids(&answer),
            expected,
            "case {case}: expected the named resource's tenants, got {}",
            answer.raw
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A `MaxItems` that names no page is refused, a `Marker` this service never handed out is
/// no resume point, and a well-formed marker naming no stored tenant is an entity that does
/// not exist.
#[tokio::test]
async fn a_request_that_names_no_page_or_no_resume_point_is_refused() {
    let router = emulator();
    let fixture = fixture(&router).await;
    // Every request below filters by one association, which is what the listing admits.
    let filter = Filter::distribution(&fixture.distributions[0]);

    // Below the admissible `MaxItems` range: the member bounds the page, so a negative count
    // bounds nothing and names no page at all.
    for (case, max_items) in [("negative", -1), ("far negative", i32::MIN)] {
        let answer =
            list_tenants(&router, &body_for(Some(&filter), None, Some(max_items))).await;
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

    // The smallest admissible page size is served.
    let answer = list_tenants(&router, &body_for(Some(&filter), None, Some(1))).await;
    assert_eq!(
        answer.status, 200,
        "case MaxItems one: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        summaries(&answer).len(),
        1,
        "case MaxItems one: expected one tenant on the page, got {}",
        answer.raw
    );

    // A marker that is no distribution tenant id at all was never a resume point.
    for (case, marker) in [
        ("a word", "next-please".to_string()),
        ("a distribution id", "E1234567890ABC".to_string()),
        ("the prefix alone", DISTRIBUTION_TENANT_ID_PREFIX.to_string()),
        ("a short body", "dt_ABC".to_string()),
        (
            "a body outside the id alphabet",
            format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", "-".repeat(27)),
        ),
        ("a tenant name", "alpha-default-one".to_string()),
    ] {
        let answer = list_tenants(&router, &body_for(Some(&filter), Some(&marker), None)).await;
        assert_eq!(
            answer.status, 400,
            "case marker {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case marker {case}: expected InvalidArgument, got {}",
            answer.raw
        );
    }

    // A marker in the shape this service mints, naming no stored tenant: the entity is not
    // there.
    let absent = format!("{DISTRIBUTION_TENANT_ID_PREFIX}{}", "Z".repeat(27));
    assert!(
        !fixture.created.iter().any(|created| created.id == absent),
        "case an absent marker: the fixture must not hold {absent}"
    );
    let answer = list_tenants(&router, &body_for(Some(&filter), Some(&absent), None)).await;
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
}
