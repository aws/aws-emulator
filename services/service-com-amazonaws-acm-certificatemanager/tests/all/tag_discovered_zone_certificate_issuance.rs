// SPDX-License-Identifier: CC0-1.0

//! The whole chain a tag-driven client walks to get a DNS-validated certificate issued:
//! find the account's own public hosted zone by the tag it put on it (Route 53's
//! `ListHostedZones` pagination plus `ListTagsForResource`), request an ACM certificate
//! for a name in the zone it found, publish the CNAME ACM handed out, wait for the change
//! to propagate, and read the certificate back as `ISSUED`.
//!
//! RULE under test: a hosted zone created and tagged in the account is enumerated by
//! `ListHostedZones` with the private flag it was created with and reports through
//! `ListTagsForResource` exactly the tags written on it — so a zone is selectable by tag
//! alone, without an id anybody knew in advance — and a certificate requested for a name
//! under a zone so discovered waits in `PENDING_VALIDATION` and reaches `ISSUED`, with
//! every domain's `ValidationStatus` `SUCCESS`, once its challenge is published there.
//!
//! The two halves are asserted TOGETHER, in one suite, because the failure this pins is
//! in their composition: a client that cannot discover its zone never issues a
//! certificate at all, and a listing or a tag read that loses the zone looks exactly like
//! an account that holds none.
//!
//! The rows span what the rule generalises over: how many zones the account holds, which
//! of them are public, how many tags each carries and under which keys, the `maxitems`
//! the discovery walk pages with (one zone per page, two, more than the account holds,
//! and no `maxitems` at all), and how the certificate's names sit against the discovered
//! apex (the apex itself, a label under it, several labels deep, a wildcard, two names at
//! once).
//!
//! Both services are driven over their real wires (ACM's `POST /` awsJson1_1 and Route
//! 53's restXml `/2013-04-01/...`) on ONE shared engine, so the zone the certificate is
//! validated in is the very zone the walk discovered. Nothing waits on a fixed sleep: the
//! propagation the change owes is polled for, and the issuance the proof owes is
//! materialised by moving the instant the service itself stored into the past.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::describe;
use crate::certificate_validation_state_machine::{arns_in_statuses, make_validation_due_now};
use crate::dns_validation_published_record::{
    assert_issuance_owed, assert_issued, create_private_zone, create_public_zone, dns_call,
    request_dns_validated, router_with_dns, submit_change, Record,
};

/// The tag this suite's client selects its zones by. Both halves are the TEST's own: a
/// marker tag is a value the account writes on its zone, so no part of it comes from the
/// API.
const MARKER_TAG: (&str, &str) = ("zone-purpose", "delegated-apex");

/// How long a `GetChange` poll may keep looking for the propagation a submitted change
/// owes. Far above the span the service itself owes it, so an expired budget is a real
/// failure rather than an artefact of a loaded host.
const PROPAGATION_BUDGET: Duration = Duration::from_secs(60);

/// How long the poll waits between two `GetChange` reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `ChangeStatus` of a change that has propagated.
const INSYNC: &str = "INSYNC";

/// One zone the account holds: its apex, whether it is PRIVATE, and the tags written on
/// it after it is created.
struct ZoneSpec {
    apex: &'static str,
    private: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// How a row's certificate names sit against the apex of the zone it discovered.
#[derive(Debug, Clone, Copy)]
enum Covered {
    /// The apex itself.
    Apex,
    /// One label under the apex.
    Label,
    /// Several labels under the apex.
    Deep,
    /// A wildcard over the apex.
    Wildcard,
    /// Two names at once: the apex and a label under it.
    ApexAndLabel,
}

impl Covered {
    /// The primary `DomainName` and the subject alternative names, for `apex`.
    fn names_of(self, apex: &str) -> (String, Vec<String>) {
        match self {
            Self::Apex => (apex.to_string(), Vec::new()),
            Self::Label => (format!("www.{apex}"), Vec::new()),
            Self::Deep => (format!("a.b.c.{apex}"), Vec::new()),
            Self::Wildcard => (format!("*.{apex}"), Vec::new()),
            Self::ApexAndLabel => (apex.to_string(), vec![format!("api.{apex}")]),
        }
    }
}

/// One row of the table: the account the walk runs against, the page size it pages with,
/// the apex of the zone the certificate is requested under, and the shape of the names it
/// covers.
struct Row {
    case: &'static str,
    account: &'static [ZoneSpec],
    /// `None` sends no `maxitems` at all, so the service's own page size applies.
    page_size: Option<i32>,
    /// The apex the certificate is requested under. Must be one the walk discovers.
    target: &'static str,
    covered: Covered,
}

/// The text inside every `<tag>…</tag>` element of `body`, in document order. The
/// delimiters carry the closing `>`, so `<Tag>` never matches `<Tags>`.
fn elements<'a>(body: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        match after.find(&close) {
            Some(end) => {
                found.push(&after[..end]);
                rest = &after[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// The text inside the FIRST `<tag>…</tag>` element of `body`, or `None` when it carries
/// none.
fn element<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    elements(body, tag).into_iter().next()
}

/// One zone as `ListHostedZones` enumerated it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Enumerated {
    /// The bare (`Z…`) id: the listing's `Id` after the last `/`, which is the id
    /// `ListTagsForResource` takes.
    id: String,
    /// The apex, without the trailing root dot the listing reports it with.
    apex: String,
    /// `Config/PrivateZone`, as reported.
    private: bool,
}

/// Create one spec's zone, write its tags, and return its bare id.
async fn create_and_tag(router: &MultiServiceRouter, spec: &ZoneSpec, case: &str) -> String {
    // The caller reference only has to be unique in the account, and the apex is.
    let reference = format!("tag-discovery/{}", spec.apex);
    let zone = if spec.private {
        create_private_zone(router, spec.apex, &reference).await
    } else {
        create_public_zone(router, spec.apex, &reference).await
    };

    if !spec.tags.is_empty() {
        let tags: String = spec
            .tags
            .iter()
            .map(|(key, value)| format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"))
            .collect();
        let body = format!(
            "<ChangeTagsForResourceRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <AddTags>{tags}</AddTags></ChangeTagsForResourceRequest>"
        );
        let (status, reply) = dns_call(
            router,
            "POST",
            &format!("/2013-04-01/tags/hostedzone/{zone}"),
            &body,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: tagging {} must be accepted, got {status} {reply}",
            spec.apex
        );
    }
    zone
}

/// Every zone `ListHostedZones` enumerates, in the order the walk met them, paged the way
/// `page_size` asks for.
///
/// The walk asserts the pagination contract it depends on as it goes: a page is never
/// longer than the `maxitems` asked for, `NextMarker` is present exactly when
/// `IsTruncated` is true, and the response echoes the `Marker` the request resumed from.
async fn enumerate(
    router: &MultiServiceRouter,
    page_size: Option<i32>,
    case: &str,
    zone_count: usize,
) -> Vec<Enumerated> {
    let mut found: Vec<Enumerated> = Vec::new();
    let mut marker: Option<String> = None;
    // One page per zone is the most a walk can need, plus the final page; a walk that
    // wants more is a marker that is not advancing.
    let page_limit = zone_count + 2;
    for page_number in 1..=page_limit {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxitems={size}"));
        }
        if let Some(at) = &marker {
            query.push(format!("marker={at}"));
        }
        let uri = if query.is_empty() {
            "/2013-04-01/hostedzone".to_string()
        } else {
            format!("/2013-04-01/hostedzone?{}", query.join("&"))
        };
        let (status, body) = dns_call(router, "GET", &uri, "").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from ListHostedZones, got {status} {body}"
        );
        // An empty element carries no text, so the first page's echo of the empty marker
        // reads as the empty string.
        assert_eq!(
            element(&body, "Marker").unwrap_or(""),
            marker.as_deref().unwrap_or(""),
            "case {case}: the response must echo the Marker it resumed from, got {body}"
        );

        let page: Vec<Enumerated> = elements(&body, "HostedZone")
            .into_iter()
            .map(|zone| {
                let id = element(zone, "Id")
                    .unwrap_or_else(|| panic!("case {case}: an enumerated zone must report its Id, got {zone}"))
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let apex = element(zone, "Name")
                    .unwrap_or_else(|| panic!("case {case}: an enumerated zone must report its Name, got {zone}"))
                    .trim_end_matches('.')
                    .to_string();
                let private = element(zone, "PrivateZone").unwrap_or_else(|| {
                    panic!("case {case}: an enumerated zone must report Config/PrivateZone, got {zone}")
                });
                Enumerated {
                    id,
                    apex,
                    private: private == "true",
                }
            })
            .collect();
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "case {case}: a page holds at most the maxitems asked for ({size}), got {body}"
            );
        }
        found.extend(page);

        let truncated = element(&body, "IsTruncated") == Some("true");
        let next = element(&body, "NextMarker").map(str::to_string);
        assert_eq!(
            truncated,
            next.is_some(),
            "case {case}: NextMarker is present exactly when IsTruncated is true, got {body}"
        );
        match next {
            Some(at) => marker = Some(at),
            None => return found,
        }
        assert!(
            page_number < page_limit,
            "case {case}: the walk did not finish within {page_limit} page(s), so a marker \
             is not advancing"
        );
    }
    found
}

/// The tags `ListTagsForResource` reports for one zone, keyed by tag key.
async fn tags_of(
    router: &MultiServiceRouter,
    zone: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = dns_call(
        router,
        "GET",
        &format!("/2013-04-01/tags/hostedzone/{zone}"),
        "",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from ListTagsForResource for {zone}, got {status} {body}"
    );
    assert_eq!(
        element(&body, "ResourceId"),
        Some(zone),
        "case {case}: the tag set must name the zone it was asked about, got {body}"
    );
    elements(&body, "Tag")
        .into_iter()
        .map(|tag| {
            let key = element(tag, "Key").unwrap_or_default().to_string();
            let value = element(tag, "Value").unwrap_or_default().to_string();
            (key, value)
        })
        .collect()
}

/// Walk the account the way a tag-driven client does and return the apex and bare id of
/// every PUBLIC zone carrying [`MARKER_TAG`], asserting as it goes that each zone reports
/// the private flag and the tags it was created with.
async fn discover_by_tag(
    router: &MultiServiceRouter,
    row: &Row,
) -> BTreeMap<String, String> {
    let enumerated = enumerate(router, row.page_size, row.case, row.account.len()).await;

    let expected: BTreeMap<&str, bool> = row
        .account
        .iter()
        .map(|spec| (spec.apex, spec.private))
        .collect();
    let walked: BTreeMap<&str, bool> = enumerated
        .iter()
        .map(|zone| (zone.apex.as_str(), zone.private))
        .collect();
    assert_eq!(
        walked.len(),
        enumerated.len(),
        "case {}: paging must enumerate each zone exactly once, got {enumerated:?}",
        row.case
    );
    assert_eq!(
        walked, expected,
        "case {}: ListHostedZones must enumerate every zone of the account with the \
         private flag it was created with",
        row.case
    );

    let mut selected = BTreeMap::new();
    for zone in &enumerated {
        let spec = row
            .account
            .iter()
            .find(|spec| spec.apex == zone.apex)
            .unwrap_or_else(|| panic!("case {}: {zone:?} is no zone of the account", row.case));
        let reported = tags_of(router, &zone.id, row.case).await;
        let written: BTreeMap<String, String> = spec
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, written,
            "case {}: ListTagsForResource must report the tags written on {}, got \
             {reported:?}",
            row.case, zone.apex
        );
        if !zone.private && reported.get(MARKER_TAG.0).map(String::as_str) == Some(MARKER_TAG.1) {
            selected.insert(zone.apex.clone(), zone.id.clone());
        }
    }

    let expected_selection: BTreeMap<String, ()> = row
        .account
        .iter()
        .filter(|spec| !spec.private && spec.tags.contains(&MARKER_TAG))
        .map(|spec| (spec.apex.to_string(), ()))
        .collect();
    let selection: BTreeMap<String, ()> = selected.keys().map(|apex| (apex.clone(), ())).collect();
    assert_eq!(
        selection, expected_selection,
        "case {}: a client selecting public zones by the marker tag must find exactly the \
         public zones it was written on",
        row.case
    );
    selected
}

/// Publish one validation record in `zone` and poll `GetChange` until the change it
/// created has propagated — the wait a client performs before it expects the name to
/// resolve.
async fn publish_and_settle(case: &str, router: &MultiServiceRouter, zone: &str, record: &Record) {
    let change = submit_change(case, router, zone, "CREATE", record).await;
    let deadline = Instant::now() + PROPAGATION_BUDGET;
    loop {
        let (status, body) = dns_call(
            router,
            "GET",
            &format!("/2013-04-01/change/{change}"),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetChange for {change}, got {status} {body}"
        );
        if element(&body, "Status") == Some(INSYNC) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the change {change} did not reach {INSYNC} within \
             {PROPAGATION_BUDGET:?}, last read {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The accounts the rows run against, each a different mix of zones.
const ONE_TAGGED_ZONE: &[ZoneSpec] = &[ZoneSpec {
    apex: "apex.tag-discovery-one.net",
    private: false,
    tags: &[MARKER_TAG],
}];

const TAGGED_AMONG_OTHERS: &[ZoneSpec] = &[
    ZoneSpec {
        apex: "apex.tag-discovery-mixed.net",
        private: false,
        tags: &[MARKER_TAG, ("Env", "prod"), ("Owner", "dns-team")],
    },
    ZoneSpec {
        apex: "untagged.tag-discovery-mixed.net",
        private: false,
        tags: &[],
    },
    ZoneSpec {
        apex: "internal.tag-discovery-mixed.org",
        private: true,
        tags: &[MARKER_TAG],
    },
    // The same tag KEY under another value: a client selects on the pair, so this zone
    // is no match either.
    ZoneSpec {
        apex: "other.tag-discovery-mixed.org",
        private: false,
        tags: &[(MARKER_TAG.0, "internal-apex")],
    },
];

const TWO_TAGGED_ZONES: &[ZoneSpec] = &[
    ZoneSpec {
        apex: "first.tag-discovery-two.io",
        private: false,
        tags: &[MARKER_TAG],
    },
    ZoneSpec {
        apex: "second.tag-discovery-two.io",
        private: false,
        tags: &[MARKER_TAG, ("Env", "staging")],
    },
    ZoneSpec {
        apex: "legacy.tag-discovery-two.io",
        private: true,
        tags: &[],
    },
];

/// RULE: a certificate requested for a name under a zone the account discovered by its
/// own tag waits for proof and reaches `ISSUED` once the challenge ACM handed out is
/// published in that zone — whatever the account holds beside it and however the walk
/// pages.
#[tokio::test]
async fn a_tag_discovered_zone_validates_a_requested_certificate() {
    let rows = [
        Row {
            case: "one tagged public zone, paged one zone at a time, a label under the apex",
            account: ONE_TAGGED_ZONE,
            page_size: Some(1),
            target: "apex.tag-discovery-one.net",
            covered: Covered::Label,
        },
        Row {
            case: "the tagged zone among an untagged, a private and an otherwise tagged \
                   zone, paged two at a time, the apex itself",
            account: TAGGED_AMONG_OTHERS,
            page_size: Some(2),
            target: "apex.tag-discovery-mixed.net",
            covered: Covered::Apex,
        },
        Row {
            case: "the tagged zone discovered without any maxitems, a wildcard over the apex",
            account: ONE_TAGGED_ZONE,
            page_size: None,
            target: "apex.tag-discovery-one.net",
            covered: Covered::Wildcard,
        },
        Row {
            case: "two tagged public zones, paged beyond what the account holds, a name \
                   several labels deep",
            account: TWO_TAGGED_ZONES,
            page_size: Some(10),
            target: "second.tag-discovery-two.io",
            covered: Covered::Deep,
        },
        Row {
            case: "the tagged zone among others, paged three at a time, two names at once",
            account: TAGGED_AMONG_OTHERS,
            page_size: Some(3),
            target: "apex.tag-discovery-mixed.net",
            covered: Covered::ApexAndLabel,
        },
    ];

    for row in rows {
        // A fresh store per row, so one row's zones or certificates cannot explain
        // another's.
        let (router, engine) = router_with_dns();
        for spec in row.account {
            create_and_tag(&router, spec, row.case).await;
        }

        let discovered = discover_by_tag(&router, &row).await;
        let zone = discovered.get(row.target).unwrap_or_else(|| {
            panic!(
                "case {}: the walk must discover {}, got {discovered:?}",
                row.case, row.target
            )
        });

        let (domain, sans) = row.covered.names_of(row.target);
        let san_refs: Vec<&str> = sans.iter().map(String::as_str).collect();
        let (arn, records) =
            request_dns_validated(row.case, &router, &domain, &san_refs).await;

        for record in &records {
            publish_and_settle(row.case, &router, zone, record).await;
        }

        // The proof is recorded, and the issuance it owes is materialised by the next call
        // once the instant the service stored has passed.
        assert_issuance_owed(row.case, &router, &engine, &arn).await;
        make_validation_due_now(&engine);

        let issued = describe(&router, &arn).await;
        assert_issued(row.case, &issued);
        assert_eq!(
            issued["DomainName"].as_str(),
            Some(domain.as_str()),
            "case {}: the requested DomainName is echoed verbatim, got {issued}",
            row.case
        );
        assert_eq!(
            issued["InUseBy"],
            json!([]),
            "case {}: nothing references a freshly issued certificate, got {issued}",
            row.case
        );
        let listed = arns_in_statuses(&router, &["ISSUED"]).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case {}: expected ListCertificates to index the certificate as ISSUED, got \
             {listed:?}",
            row.case
        );
    }
}

/// RULE's other half: both discovery reads are REPORTING calls, so the chain is
/// repeatable — walking the account again after the certificate issued reports the same
/// zones and the same tags, and the certificate stays `ISSUED`.
#[tokio::test]
async fn the_discovery_reads_report_the_same_account_after_issuance() {
    let row = Row {
        case: "the discovery walked again after the certificate issued",
        account: TAGGED_AMONG_OTHERS,
        page_size: Some(2),
        target: "apex.tag-discovery-mixed.net",
        covered: Covered::Label,
    };

    let (router, engine) = router_with_dns();
    for spec in row.account {
        create_and_tag(&router, spec, row.case).await;
    }

    let before = enumerate(&router, row.page_size, row.case, row.account.len()).await;
    let discovered = discover_by_tag(&router, &row).await;
    let zone = discovered
        .get(row.target)
        .unwrap_or_else(|| panic!("case {}: the walk must discover {}", row.case, row.target));

    let (domain, _) = row.covered.names_of(row.target);
    let (arn, records) = request_dns_validated(row.case, &router, &domain, &[]).await;
    for record in &records {
        publish_and_settle(row.case, &router, zone, record).await;
    }
    assert_issuance_owed(row.case, &router, &engine, &arn).await;
    make_validation_due_now(&engine);
    assert_issued(row.case, &describe(&router, &arn).await);

    let after = enumerate(&router, row.page_size, row.case, row.account.len()).await;
    assert_eq!(
        before, after,
        "case {}: two walks over ListHostedZones must enumerate the same zones in the \
         same order",
        row.case
    );
    let again = discover_by_tag(&router, &row).await;
    assert_eq!(
        again, discovered,
        "case {}: a second tag-driven discovery must select the same zones",
        row.case
    );

    let still = describe(&router, &arn).await;
    assert_eq!(
        still["Status"].as_str(),
        Some("ISSUED"),
        "case {}: an issued certificate stays issued across further reads, got {still}",
        row.case
    );
}
