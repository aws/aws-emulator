// SPDX-License-Identifier: CC0-1.0

//! Listing distribution tenants BY CUSTOMIZATION, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * a tenant is reported by a customization listing exactly when its OWN stored
//!   `Customizations` names the ARN the request filters by — so the listing follows the
//!   record a create, or a later association, persisted, and a tenant that customizes the
//!   other kind, that disables the setting it inherits (which names no ARN), or that
//!   carries no customization at all is reported by neither filter;
//! * the ARN is the identity of a resource of ANOTHER service (ACM, WAF), so it is matched
//!   whole and in every form an ARN of that service takes, and an ARN no tenant names is an
//!   empty page rather than a refusal;
//! * "You must specify either the CertificateArn parameter or WebACLArn parameter, but not
//!   both in the same request": a request that names neither, or both, is `InvalidArgument`
//!   (HTTP 400);
//! * the listing reports the matching tenants in ONE stable total order, cut to at most
//!   `MaxItems` entries beginning after the `Marker` id, and carries a `NextMarker` exactly
//!   when matching tenants remain — so walking the pages at ANY admissible page size
//!   enumerates them exactly once;
//! * a `MaxItems` that names no page, and a `Marker` this service never handed out, are
//!   refused; a well-formed marker naming no stored tenant is an entity that does not
//!   exist;
//! * each summary reports the same values a read of that one tenant reports, down to its
//!   version token — one record, two readers.
//!
//! The rows differ along what the rules generalise over: the FORM of the ARN filtered by
//! (every entry of [`crate::web_acl_association::WEB_ACLS`] — the WAF generation, the
//! account, the partition and the bare WAF Classic id — and three ACM certificate ARNs
//! spanning regions and partitions), the KIND of customization a tenant carries (a web ACL
//! overridden, a web ACL disabled, a certificate, both, geographic restrictions only, none
//! at all), how many tenants share one ARN, and every page size from one up past the
//! matching set, plus this service's own page ceiling and one past it.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, and the tenants are
//! created through [`crate::distribution_tenant_lifecycle`]'s helpers, so these assertions
//! travel the same path a client takes.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    DISTRIBUTION_TENANT_ID_PREFIX, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, domains_of, escape, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::response_headers_policy_lifecycle::{assert_absent, text};
use crate::tenant_web_acl_association::{associate, disassociate};
use crate::web_acl_association::WEB_ACLS;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/distribution-tenants-by-customization` with the request document
/// `body`.
async fn list_by_customization(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution-tenants-by-customization"),
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

/// A `ListDistributionTenantsByCustomizationRequest` document naming whichever of the four
/// input members the case under test names, in the order the shape declares them.
fn list_body(
    web_acl_arn: Option<&str>,
    certificate_arn: Option<&str>,
    marker: Option<&str>,
    max_items: Option<i32>,
) -> String {
    let mut out = format!(
        "<ListDistributionTenantsByCustomizationRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(web_acl_arn) = web_acl_arn {
        out.push_str(&format!("<WebACLArn>{}</WebACLArn>", escape(web_acl_arn)));
    }
    if let Some(certificate_arn) = certificate_arn {
        out.push_str(&format!(
            "<CertificateArn>{}</CertificateArn>",
            escape(certificate_arn)
        ));
    }
    if let Some(marker) = marker {
        out.push_str(&format!("<Marker>{}</Marker>", escape(marker)));
    }
    if let Some(max_items) = max_items {
        out.push_str(&format!("<MaxItems>{max_items}</MaxItems>"));
    }
    out.push_str("</ListDistributionTenantsByCustomizationRequest>");
    out
}

/// The `DistributionTenantSummary` elements one listing answered with.
///
/// `DistributionTenantList`'s member carries `xmlName` `DistributionTenantSummary`
/// (`smithy_query(query="shape",shape="DistributionTenantList")`), so each entry travels in
/// an element of that name inside the `DistributionTenantList` wrapper.
fn summaries(answer: &Answer) -> Vec<&Node> {
    match answer.root().at("DistributionTenantList") {
        Some(wrapper) => wrapper.children_named("DistributionTenantSummary"),
        None => Vec::new(),
    }
}

/// The ids one listing reported, in the order it reported them.
fn summary_ids(answer: &Answer) -> Vec<String> {
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
// The customizations under test
// ---------------------------------------------------------------------------

/// Three ACM certificate ARNs, in the form AWS writes one in.
///
/// The form comes from the model's own `Certificate$Arn` — "the Amazon Resource Name (ARN)
/// of the ACM certificate"
/// (`smithy_query(query="shape",shape="Certificate")`) — and the rows span the region, the
/// account and the partition segments, because an ARN is matched whole.
const CERTIFICATES: &[(&str, &str)] = &[
    (
        "acm-us-east-1",
        "arn:aws:acm:us-east-1:123456789012:certificate/\
         12345678-1234-1234-1234-123456789012",
    ),
    (
        "acm-eu-west-1",
        "arn:aws:acm:eu-west-1:210987654321:certificate/\
         abcdefab-cdef-abcd-efab-cdefabcdefab",
    ),
    (
        "acm-other-partition",
        "arn:aws-cn:acm:cn-north-1:123456789012:certificate/\
         99998888-7777-6666-5555-444433332222",
    ),
];

/// One distribution tenant the table creates.
#[derive(Debug, Clone, Copy)]
struct Tenant {
    case: &'static str,
    /// Names the tenant and its domain, both unique in the account.
    slug: &'static str,
    /// `Customizations$Certificate$Arn`, or `None` for a tenant that customizes no
    /// certificate.
    certificate_arn: Option<&'static str>,
    /// `Customizations$WebAcl` as `(Action, Arn)` — the ARN is absent on the `disable`
    /// action, which names no web ACL — or `None` for a tenant that customizes no web ACL.
    web_acl: Option<(&'static str, Option<&'static str>)>,
    /// Whether the create also names geographic restrictions, a customization no filter here
    /// is about.
    geo: bool,
}

impl Tenant {
    /// The `Customizations` element this row submits, or the empty string for a row that
    /// names no customizations at all.
    fn customizations_xml(&self) -> String {
        if self.certificate_arn.is_none() && self.web_acl.is_none() && !self.geo {
            return String::new();
        }
        let mut out = String::from("<Customizations>");
        if let Some((action, arn)) = self.web_acl {
            out.push_str(&format!("<WebAcl><Action>{action}</Action>"));
            if let Some(arn) = arn {
                out.push_str(&format!("<Arn>{}</Arn>", escape(arn)));
            }
            out.push_str("</WebAcl>");
        }
        if let Some(arn) = self.certificate_arn {
            out.push_str(&format!("<Certificate><Arn>{}</Arn></Certificate>", escape(arn)));
        }
        if self.geo {
            out.push_str(
                "<GeoRestrictions><RestrictionType>whitelist</RestrictionType>\
                 <Locations><Location>DE</Location></Locations></GeoRestrictions>",
            );
        }
        out.push_str("</Customizations>");
        out
    }

    /// The `CreateDistributionTenantRequest` XML for this row, under `distribution_id`.
    fn xml(&self, distribution_id: &str) -> String {
        format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{}</DistributionId><Name>{}</Name>\
             <Domains><member><Domain>{}.example.com</Domain></member></Domains>\
             <Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>\
             {}{}</CreateDistributionTenantRequest>",
            escape(distribution_id),
            self.slug,
            self.slug,
            self.slug,
            self.customizations_xml(),
            managed_certificate_request()
        )
    }

    /// Whether this tenant's record names `arn` as the customization of `kind` — the rule
    /// each filter below is asserted against, stated once.
    fn carries(&self, kind: Kind, arn: &str) -> bool {
        match kind {
            Kind::Certificate => self.certificate_arn == Some(arn),
            Kind::WebAcl => self.web_acl.and_then(|(_, named)| named) == Some(arn),
        }
    }
}

/// The kind of customization a request filters by: the two alternatives the input offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Certificate,
    WebAcl,
}

/// The tenants every case below is asserted over, spanning each kind of customization, each
/// ARN form of the two catalogs, and several tenants sharing one ARN.
fn tenants() -> Vec<Tenant> {
    let mut rows = vec![
        Tenant {
            case: "no customizations at all",
            slug: "plain-tenant",
            certificate_arn: None,
            web_acl: None,
            geo: false,
        },
        Tenant {
            case: "geographic restrictions only",
            slug: "geo-tenant",
            certificate_arn: None,
            web_acl: None,
            geo: true,
        },
        Tenant {
            case: "the web ACL disabled, which names no ARN",
            slug: "web-disabled-tenant",
            certificate_arn: None,
            web_acl: Some(("disable", None)),
            geo: false,
        },
        Tenant {
            case: "a second tenant sharing the first web ACL",
            slug: "web-twin-tenant",
            certificate_arn: None,
            web_acl: Some(("override", Some(WEB_ACLS[0].1))),
            geo: true,
        },
        Tenant {
            case: "a second tenant sharing the first certificate",
            slug: "cert-twin-tenant",
            certificate_arn: Some(CERTIFICATES[0].1),
            web_acl: None,
            geo: false,
        },
        Tenant {
            case: "both a certificate and a web ACL",
            slug: "both-tenant",
            certificate_arn: Some(CERTIFICATES[0].1),
            web_acl: Some(("override", Some(WEB_ACLS[0].1))),
            geo: false,
        },
    ];
    // One tenant per web ACL form, so every form the catalog states is both stored and
    // filtered by.
    for (case, arn) in WEB_ACLS {
        rows.push(Tenant {
            case,
            slug: case,
            certificate_arn: None,
            web_acl: Some(("override", Some(arn))),
            geo: false,
        });
    }
    // And one per certificate form, for the same reason.
    for (case, arn) in CERTIFICATES {
        rows.push(Tenant {
            case,
            slug: case,
            certificate_arn: Some(arn),
            web_acl: None,
            geo: false,
        });
    }
    rows
}

/// One tenant as the create that stored it left it: the row, its minted id and its version
/// token.
#[derive(Debug, Clone)]
struct Created {
    tenant: Tenant,
    id: String,
    etag: String,
}

/// Create the multi-tenant distribution and every tenant of [`tenants`] under it.
async fn create_every_tenant(router: &MultiServiceRouter) -> Vec<Created> {
    let distribution_id = multi_tenant_distribution(router, "ref-by-customization").await;
    let mut created = Vec::new();
    for tenant in tenants() {
        let answer = create_distribution_tenant(router, &tenant.xml(&distribution_id)).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateDistributionTenant, got {} with {}",
            tenant.case, answer.status, answer.raw
        );
        created.push(Created {
            tenant,
            id: text(answer.root(), "Id", tenant.case, "the created tenant", &answer.raw),
            etag: answer.header("etag").to_string(),
        });
    }
    created
}

/// Every filter the cases below run: each ARN of both catalogs, plus one ARN of each kind
/// that no tenant names.
fn filters() -> Vec<(String, Kind, String)> {
    let mut rows: Vec<(String, Kind, String)> = Vec::new();
    for (case, arn) in WEB_ACLS {
        rows.push((format!("web ACL {case}"), Kind::WebAcl, arn.to_string()));
    }
    for (case, arn) in CERTIFICATES {
        rows.push((
            format!("certificate {case}"),
            Kind::Certificate,
            arn.to_string(),
        ));
    }
    rows.push((
        "a web ACL no tenant customizes".to_string(),
        Kind::WebAcl,
        "arn:aws:wafv2:us-east-1:123456789012:global/webacl/Unused/\
         00000000-0000-0000-0000-EXAMPLE00000"
            .to_string(),
    ));
    rows.push((
        "a certificate no tenant customizes".to_string(),
        Kind::Certificate,
        "arn:aws:acm:us-east-1:123456789012:certificate/\
         00000000-0000-0000-0000-000000000000"
            .to_string(),
    ));
    rows
}

/// The request one filter row sends, under `marker` and `max_items`.
fn body_for(kind: Kind, arn: &str, marker: Option<&str>, max_items: Option<i32>) -> String {
    match kind {
        Kind::WebAcl => list_body(Some(arn), None, marker, max_items),
        Kind::Certificate => list_body(None, Some(arn), marker, max_items),
    }
}

/// The ids the table says one filter must report, in the order the service reports them
/// (tenant id order).
fn expected_ids(created: &[Created], kind: Kind, arn: &str) -> Vec<String> {
    let mut ids: Vec<String> = created
        .iter()
        .filter(|row| row.tenant.carries(kind, arn))
        .map(|row| row.id.clone())
        .collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// The filter
// ---------------------------------------------------------------------------

/// Each filter reports exactly the tenants whose own record names that ARN, and nothing
/// else: an ARN no tenant customizes is an empty page, and a tenant that disables its web
/// ACL, customizes only the other kind, or customizes nothing is reported by neither.
#[tokio::test]
async fn each_filter_reports_the_tenants_whose_record_names_that_arn() {
    let router = emulator();
    let created = create_every_tenant(&router).await;

    for (case, kind, arn) in filters() {
        let answer = list_by_customization(&router, &body_for(kind, &arn, None, None)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        let expected = expected_ids(&created, kind, &arn);
        let got = summary_ids(&answer);
        assert_eq!(
            got, expected,
            "case {case}: expected the tenants carrying that customization, got {got:?} with {}",
            answer.raw
        );
        // The whole matching set fits one page, so nothing remains to resume from.
        assert_absent(answer.root(), "NextMarker", &case, "the only page");
    }

    // Every tenant the table stored that customizes nothing a filter is about is reported by
    // no filter at all.
    let reported: BTreeSet<String> = filters()
        .into_iter()
        .flat_map(|(_, kind, arn)| expected_ids(&created, kind, &arn))
        .collect();
    for row in &created {
        let customized = row.tenant.certificate_arn.is_some()
            || row.tenant.web_acl.and_then(|(_, arn)| arn).is_some();
        assert_eq!(
            reported.contains(&row.id),
            customized,
            "case {}: a tenant is reported exactly when its record names an ARN, got \
             reported={}",
            row.tenant.case,
            reported.contains(&row.id)
        );
    }
}

/// Each summary reports the same values a read of that one tenant reports — one record,
/// two readers — including the version token the create handed back, and carries no member
/// the summary shape does not declare.
#[tokio::test]
async fn each_summary_reports_what_a_read_of_the_same_tenant_reports() {
    let router = emulator();
    let created = create_every_tenant(&router).await;

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
        "Customizations/WebAcl/Action",
        "Customizations/WebAcl/Arn",
        "Customizations/Certificate/Arn",
        "Customizations/GeoRestrictions/RestrictionType",
    ];

    for (case, kind, arn) in filters() {
        let answer = list_by_customization(&router, &body_for(kind, &arn, None, None)).await;
        for summary in summaries(&answer) {
            let id = text(summary, "Id", &case, "a summary", &answer.raw);
            let row = created
                .iter()
                .find(|row| row.id == id)
                .unwrap_or_else(|| panic!("case {case}: the listing reported an unknown id {id}"));
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
                    "case {case} (tenant {}): the summary and the read disagree about {path}: \
                     {} against {}",
                    row.tenant.case,
                    answer.raw,
                    read.raw
                );
            }
            assert_eq!(
                domains_of(summary),
                domains_of(read.root()),
                "case {case} (tenant {}): the summary and the read disagree about Domains: {}",
                row.tenant.case,
                answer.raw
            );
            // `DistributionTenantSummary$ETag` is "the current version of the distribution
            // tenant": the token the create handed back, which the read reports as its
            // header.
            assert_eq!(
                summary.text_at("ETag"),
                Some(row.etag.as_str()),
                "case {case} (tenant {}): expected the tenant's current version in the \
                 summary, got {}",
                row.tenant.case,
                answer.raw
            );
            assert_eq!(
                read.header("etag"),
                row.etag,
                "case {case} (tenant {}): expected the read to report the same version, got {}",
                row.tenant.case,
                read.raw
            );
            // The summary shape declares no `Parameters`, so the parameter values a tenant
            // holds are reached through a read of the tenant itself.
            assert_absent(summary, "Parameters", &case, "a summary");
            assert!(
                read.root().at("Parameters").is_some(),
                "case {case} (tenant {}): the read carries the tenant's parameters, got {}",
                row.tenant.case,
                read.raw
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Walk every page of one filter's listing at `page_size` and answer the ids in the order
/// they were reported.
async fn walk_pages(
    router: &MultiServiceRouter,
    kind: Kind,
    arn: &str,
    page_size: i32,
    case: &str,
) -> Vec<String> {
    let case = format!("{case} at page size {page_size}");
    let mut ids: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..64 {
        let body = body_for(kind, arn, marker.as_deref(), Some(page_size));
        let answer = list_by_customization(router, &body).await;
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
/// `NextMarker` exactly when matching tenants remain — so walking the pages at ANY
/// admissible page size enumerates the matching set exactly once, in one stable order.
#[tokio::test]
async fn the_listing_pages_and_resumes_from_the_marker() {
    let router = emulator();
    let created = create_every_tenant(&router).await;

    // The two filters several tenants share, so a page really is cut: the first web ACL and
    // the first certificate.
    for (case, kind, arn) in [
        ("the shared web ACL", Kind::WebAcl, WEB_ACLS[0].1),
        ("the shared certificate", Kind::Certificate, CERTIFICATES[0].1),
    ] {
        let all = expected_ids(&created, kind, arn);
        assert!(
            all.len() > 1,
            "case {case}: the table must make several tenants share this ARN, got {all:?}"
        );

        // One row per page size from the smallest admissible one to one past the matching
        // set's size, plus the largest size this service serves and one past it.
        for page_size in [1, 2, 3, 4, MAX_ITEMS, MAX_ITEMS + 1] {
            let walked = walk_pages(&router, kind, arn, page_size, case).await;
            assert_eq!(
                walked, all,
                "case {case} at page size {page_size}: expected every matching tenant exactly \
                 once in id order, got {walked:?}"
            );
        }

        // The marker the last page ended at resumes AFTER it: nothing is left to report.
        let last = all.last().expect("the filter matches a tenant").clone();
        let answer =
            list_by_customization(&router, &body_for(kind, arn, Some(&last), None)).await;
        assert_eq!(
            answer.status, 200,
            "case {case} resumed past the last match: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert!(
            summaries(&answer).is_empty(),
            "case {case} resumed past the last match: expected nothing after the last one, got {}",
            answer.raw
        );

        // A marker that names a stored tenant the filter does NOT match is still a resume
        // point: it says where to start, not what to report.
        let unmatched = created
            .iter()
            .find(|row| !row.tenant.carries(kind, arn))
            .expect("the table stores tenants this filter does not match");
        let answer =
            list_by_customization(&router, &body_for(kind, arn, Some(&unmatched.id), None)).await;
        let expected: Vec<String> = all
            .iter()
            .filter(|id| *id > &unmatched.id)
            .cloned()
            .collect();
        assert_eq!(
            summary_ids(&answer),
            expected,
            "case {case} resumed from an unmatched tenant: expected the matches after it, got {}",
            answer.raw
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The sentence CloudFront answers a request that does not name exactly one customization,
/// written out as the LITERAL the operation's documented rule states rather than read back
/// off the crate's own constant, so a change to that constant fails this case.
const ONE_CUSTOMIZATION_REQUIRED: &str =
    "You must specify either the CertificateArn parameter or WebACLArn parameter, but not \
     both in the same request.";

/// A request must name exactly ONE customization to filter by: naming neither, naming both,
/// and naming a member that carries no value at all are each `InvalidArgument` (HTTP 400)
/// with that one sentence.
#[tokio::test]
async fn a_request_that_does_not_name_exactly_one_customization_is_refused() {
    let router = emulator();
    let created = create_every_tenant(&router).await;
    let web_acl = WEB_ACLS[0].1;
    let certificate = CERTIFICATES[0].1;

    for (case, body) in [
        ("neither member named", list_body(None, None, None, None)),
        (
            "both members named",
            list_body(Some(web_acl), Some(certificate), None, None),
        ),
        (
            "an empty web ACL element alone",
            list_body(Some(""), None, None, None),
        ),
        (
            "an empty certificate element alone",
            list_body(None, Some(""), None, None),
        ),
        (
            "both members empty",
            list_body(Some(""), Some(""), None, None),
        ),
        (
            "neither member named, but a page size",
            list_body(None, None, None, Some(5)),
        ),
    ] {
        let answer = list_by_customization(&router, &body).await;
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
            Some(ONE_CUSTOMIZATION_REQUIRED),
            "case {case}: expected the documented sentence, got {}",
            answer.raw
        );
    }

    // A member that carries NO value names nothing, so it specifies no customization
    // either: a request naming one ARN beside an empty other is the listing of the ARN it
    // named, not a request that named two.
    for (case, kind, arn, body) in [
        (
            "the web ACL named, the certificate empty",
            Kind::WebAcl,
            web_acl,
            list_body(Some(web_acl), Some(""), None, None),
        ),
        (
            "the certificate named, the web ACL empty",
            Kind::Certificate,
            certificate,
            list_body(Some(""), Some(certificate), None, None),
        ),
    ] {
        let answer = list_by_customization(&router, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            summary_ids(&answer),
            expected_ids(&created, kind, arn),
            "case {case}: expected the listing of the ARN that was named, got {}",
            answer.raw
        );
    }
}

/// A `MaxItems` that names no page is refused, a `Marker` this service never handed out is
/// no resume point, and a well-formed marker naming no stored tenant is an entity that does
/// not exist.
#[tokio::test]
async fn a_request_that_names_no_page_is_refused() {
    let router = emulator();
    let created = create_every_tenant(&router).await;
    let web_acl = WEB_ACLS[0].1;

    // Below the admissible `MaxItems` range: the member bounds the page, so a negative count
    // bounds nothing and names no page at all.
    for (case, max_items) in [("negative", -1), ("far negative", i32::MIN)] {
        let answer =
            list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, None, Some(max_items)))
                .await;
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
    let answer =
        list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, None, Some(1))).await;
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
        ("a tenant name", "plain-tenant".to_string()),
    ] {
        let answer =
            list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, Some(&marker), None))
                .await;
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
        !created.iter().any(|row| row.id == absent),
        "case an absent marker: the fixture must not hold {absent}"
    );
    let answer =
        list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, Some(&absent), None))
            .await;
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

// ---------------------------------------------------------------------------
// The listing follows the record, not the request
// ---------------------------------------------------------------------------

/// A web ACL written by a later CALL is what the listing reports: associating one with a
/// tenant that had no customizations puts it in that web ACL's listing, and disassociating
/// it takes it out again — so the filter reads the stored record rather than anything about
/// the create that first stored it.
#[tokio::test]
async fn a_tenant_joins_and_leaves_a_listing_as_its_record_changes() {
    let router = emulator();
    let created = create_every_tenant(&router).await;
    let plain = created
        .iter()
        .find(|row| row.tenant.slug == "plain-tenant")
        .expect("the table creates a tenant with no customizations")
        .clone();

    // The web ACL the association names is one the table already stored a tenant under, so
    // the listing it joins starts as whatever the creates put there — read off the table
    // rather than assumed.
    let web_acl = WEB_ACLS[1].1;
    let before = expected_ids(&created, Kind::WebAcl, web_acl);
    let answer = list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, None, None)).await;
    assert_eq!(
        summary_ids(&answer),
        before,
        "case before the association: expected the tenants the creates stored, got {}",
        answer.raw
    );

    let associated = associate(&router, &plain.id, web_acl, Some(&plain.etag)).await;
    assert_eq!(
        associated.status, 200,
        "case the association: expected HTTP 200, got {} with {}",
        associated.status, associated.raw
    );
    let after_etag = associated.header("etag").to_string();

    let answer = list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, None, None)).await;
    let expected: Vec<String> = {
        let mut ids = before.clone();
        ids.push(plain.id.clone());
        ids.sort();
        ids
    };
    assert_eq!(
        summary_ids(&answer),
        expected,
        "case after the association: expected the tenant to join that web ACL's listing, got {}",
        answer.raw
    );
    let joined = summaries(&answer)
        .into_iter()
        .find(|summary| summary.text_at("Id") == Some(plain.id.as_str()))
        .expect("the tenant joined the listing");
    assert_eq!(
        joined.text_at("ETag"),
        Some(after_etag.as_str()),
        "case after the association: expected the version the association handed back, got {}",
        answer.raw
    );

    let disassociated = disassociate(&router, &plain.id, Some(&after_etag)).await;
    assert_eq!(
        disassociated.status, 200,
        "case the disassociation: expected HTTP 200, got {} with {}",
        disassociated.status, disassociated.raw
    );
    let answer = list_by_customization(&router, &body_for(Kind::WebAcl, web_acl, None, None)).await;
    assert_eq!(
        summary_ids(&answer),
        before,
        "case after the disassociation: expected the tenant to leave that web ACL's listing, \
         got {}",
        answer.raw
    );
}
