// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the REGION a resource belongs to.
//!
//! The RULE under test, asserted over tables rather than the one pair of Regions the
//! issue recorded: this service's endpoint is REGIONAL
//! (`smithy_query(query="service_info", model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! reports `endpointPrefix`/`arnNamespace` `health-agent` with `standardRegionalEndpoints`,
//! and every modelled ARN pattern carries a `[a-z0-9-]+` region segment), so a resource
//! belongs to the Region whose endpoint created it. A request addressed to one Region
//! therefore sees that Region's inventory and no other: it enumerates only its own
//! resources, resolves only its own identifiers — answering the modelled
//! `ResourceNotFoundException` (404) for one belonging to another Region, whether the
//! call would read, mutate or delete it, except on the patient-insights runtime endpoint,
//! where a START whose `domainId` resolves to no live domain of the Region is refused by
//! that endpoint's own authorization step (401 `AccessDeniedException`) — and counts
//! quotas over its own resources alone.
//!
//! Every case drives ONE emulator (one shared store) and varies only the Region the
//! request is signed for, which is the only way the scope can be observed at all: two
//! emulators would have two stores and so could prove nothing.
//!
//! The rows differ along what the rule generalises over: which Region creates and which
//! reads (every ordered pair of the table), how many resources a Region holds (none
//! through several), which operation resolves the identifier (every operation that
//! resolves a `domainId` or a resource ARN), which resource kind the identifier names (a
//! Domain, a Subscription, a patient insights job, a listening-session scope), whether
//! the call reports or mutates, and how the page is drawn (one full page, or walked one
//! element at a time through `nextToken`).
//!
//! The harness (`Emulator`, `error_code`, `string_at`) and the tagging path helper are
//! the sibling suites': one way of driving this service through signed restJson1
//! requests, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::service::error_detail::{
    domain_not_found_message, resource_not_found_message, subscription_not_found_message,
    DOMAIN_NOT_AUTHORIZED_MESSAGE,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_DOMAINS_HELD, MAX_DOMAINS_PER_ACCOUNT, MAX_DOMAIN_REGIONS,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::resource_tagging::tags_path;

/// The Regions every table below spans. Each satisfies the `[a-z0-9-]+` region segment
/// the model's ARN patterns declare, and they differ in the ways a region code can
/// (continent prefix, direction word, index), so no row depends on the pair the issue
/// recorded.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-1", "ap-south-1", "ca-central-1"];

/// The Region [`service_com_amazonaws_connecthealth_connecthealth::state::region_of`]
/// falls back to when a request's credential scope names none that fits the modelled
/// region segment. It is the Region such a request's ARN carries, and so — by the rule —
/// the Region that request's resources belong to.
const FALLBACK_REGION: &str = "us-east-1";

/// A credential scope whose region field cannot be a region: uppercase and underscored,
/// so it fails the `[a-z0-9-]+` segment every ARN pattern in this model declares.
const UNUSABLE_REGION_SCOPE: &str = "US_WEST_2";

/// The modelled 404 a cross-Region lookup answers with
/// (`smithy_query(query="operation", operation="GetDomain")`:
/// `ResourceNotFoundException`, `httpError` 404).
const NOT_FOUND: (u16, &str) = (404, "ResourceNotFoundException");

/// What the patient-insights RUNTIME endpoint answers instead when the `domainId` a START
/// addresses resolves to no domain this Region holds live: its own authorization failure
/// (`smithy_query(query="operation", operation="StartPatientInsightsJob")`:
/// `AccessDeniedException`, `httpError` 401), whose sentence names nothing read off the
/// request.
const NOT_AUTHORIZED: (u16, &str) = (401, "AccessDeniedException");

/// One row of the cross-Region table below: the operation, the method and path it is
/// addressed with, the body it carries, the detail text it must answer with, and the
/// refusal (status and error code) it must earn.
type CrossRegionRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    String,
    (u16, &'static str),
);

/// The region segment of an ARN of this service
/// (`arn:aws:health-agent:<region>:<account>:<resource>`).
fn arn_region(arn: &str) -> &str {
    arn.split(':').nth(3).unwrap_or_default()
}

/// The `domainId`s `ListDomains` reports to a caller signed for `region`, drawn as ONE
/// page.
async fn listed_domain_ids(emulator: &Emulator, region: &str, query: &str) -> BTreeSet<String> {
    let (status, page) = emulator
        .send_in(region, "GET", &format!("/domain{query}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {region:?}{query}: ListDomains must answer 200, got {page}"
    );
    let summaries = page
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {region:?}{query}: domains must be a list, got {page}")
        });
    for summary in summaries {
        let arn = string_at(summary, "arn");
        assert_eq!(
            arn_region(&arn),
            region,
            "case {region:?}{query}: a summary's ARN must name the Region that serves it, \
             got {arn:?}"
        );
    }
    summaries
        .iter()
        .map(|summary| string_at(summary, "domainId"))
        .collect()
}

/// The `domainId`s `ListDomains` reports to a caller signed for `region` when the page is
/// walked one element at a time: the cursor must stay inside the Region too.
async fn paged_domain_ids(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    // At most one step per domain the whole store could hold for this account, so a
    // cursor that failed to advance cannot loop forever.
    for _ in 0..=(MAX_DOMAINS_PER_ACCOUNT * REGIONS.len()) {
        let path = match &token {
            None => "/domain?maxResults=1".to_string(),
            Some(token) => format!("/domain?maxResults=1&nextToken={token}"),
        };
        let (status, page) = emulator.send_in(region, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {region:?}: paged ListDomains must answer 200, got {page}"
        );
        for summary in page
            .get("domains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {region:?}: domains must be a list, got {page}"))
        {
            let id = string_at(summary, "domainId");
            assert!(
                seen.insert(id.clone()),
                "case {region:?}: {id} was paged twice"
            );
        }
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return seen,
        }
    }
    panic!("case {region:?}: paging never terminated after {seen:?}")
}

/// A `StartPatientInsightsJob` body carrying only the required members.
fn start_job_body() -> Value {
    json!({
        "patientContext": { "patientId": "patient-0001" },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Annual checkup" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": { "s3Sources": [ { "uri": "s3://clinic-input/patient-0001/" } ] },
        "outputDataConfig": { "s3OutputPath": "s3://clinic-output/insights/" },
    })
}

/// A well-formed `ScribeSessionId` (`@length 36..36`, pattern
/// `[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}`).
const SESSION_ID: &str = "7b2f1c4a-9d3e-4f56-8a1b-0c2d3e4f5a6b";

#[tokio::test]
async fn each_region_lists_only_the_domains_created_in_it() {
    // The rows differ along how many domains a Region holds — none, one, and several —
    // so an empty Region is asserted as explicitly as a populated one.
    let counts: [usize; 4] = [0, 1, 2, 3];
    let emulator = Emulator::new(REGIONS[0]);

    let mut created: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (region, count) in REGIONS.iter().zip(counts) {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for index in 0..count {
            let created = emulator
                .create_domain_in(region, &json!({ "name": format!("clinic-{region}-{index}") }))
                .await;
            let arn = string_at(&created, "arn");
            assert_eq!(
                arn_region(&arn),
                *region,
                "case {region:?}: the created ARN must name the Region that minted it, \
                 got {arn:?}"
            );
            ids.insert(string_at(&created, "domainId"));
        }
        created.push((region, ids));
    }

    for (region, own) in &created {
        for query in ["", "?status=ACTIVE", "?maxResults=100"] {
            let listed = listed_domain_ids(&emulator, region, query).await;
            assert_eq!(
                &listed, own,
                "case {region:?}{query}: ListDomains must report exactly this Region's \
                 domains, got {listed:?}"
            );
        }
        let paged = paged_domain_ids(&emulator, region).await;
        assert_eq!(
            &paged, own,
            "case {region:?}: paging one domain at a time must stay inside the Region, \
             got {paged:?}"
        );
    }

    // And the identifiers themselves do not cross: every ordered pair of Regions.
    for (owner, own) in &created {
        for domain_id in own {
            for (other, _) in created.iter().filter(|(region, _)| region != owner) {
                let (status, body) = emulator
                    .send_in(other, "GET", &format!("/domain/{domain_id}"), None)
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    NOT_FOUND,
                    "case {owner:?} -> {other:?}: GetDomain of a domain of another Region \
                     must answer the modelled 404, got {status} {body}"
                );
            }
            // Its own Region still resolves it, so the refusals above are about the
            // Region and not about the id.
            let (status, body) = emulator
                .send_in(owner, "GET", &format!("/domain/{domain_id}"), None)
                .await;
            assert_eq!(
                status, 200,
                "case {owner:?}: its own Region must resolve {domain_id}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn every_operation_that_resolves_a_domain_refuses_one_of_another_region() {
    let home = REGIONS[1];
    let emulator = Emulator::new(home);

    let tags = json!({ "env": "test" });
    let created = emulator
        .create_domain_in(home, &json!({ "name": "clinic-home", "tags": tags }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let domain_arn = string_at(&created, "arn");

    let (status, subscription) = emulator
        .send_in(
            home,
            "POST",
            &format!("/domains/{domain_id}/subscriptions"),
            Some(&json!({})),
        )
        .await;
    assert_eq!(status, 201, "CreateSubscription got {subscription}");
    let subscription_id = string_at(&subscription, "subscriptionId");
    let subscription_arn = string_at(&subscription, "arn");

    let (status, job) = emulator
        .send_in(
            home,
            "POST",
            &format!("/domain/{domain_id}/patient-insights-job"),
            Some(&start_job_body()),
        )
        .await;
    assert_eq!(status, 200, "StartPatientInsightsJob got {job}");
    let job_id = string_at(&job, "jobId");

    // One row per operation that resolves a `domainId` or a resource ARN, reporting and
    // mutating alike, with the refusal and the detail text each answers for the identifier
    // it could not resolve. `DeleteDomain` is included: a Region that does not hold the
    // domain must not be able to delete it either. Every row but one answers the modelled
    // 404 naming the identifier; `StartPatientInsightsJob` is served by the patient-insights
    // RUNTIME endpoint, which refuses a `domainId` it cannot resolve a live domain for with
    // its own authorization failure and a sentence that names nothing from the request — a
    // domain of another Region being one such id.
    let rows: Vec<CrossRegionRow> = vec![
        (
            "GetDomain",
            "GET",
            format!("/domain/{domain_id}"),
            None,
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
        (
            "ListSubscriptions",
            "GET",
            format!("/domains/{domain_id}/subscriptions"),
            None,
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
        (
            "CreateSubscription",
            "POST",
            format!("/domains/{domain_id}/subscriptions"),
            Some(json!({})),
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
        (
            "GetSubscription",
            "GET",
            format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
            subscription_not_found_message(&domain_id, &subscription_id),
            NOT_FOUND,
        ),
        (
            "ActivateSubscription",
            "POST",
            format!("/domains/{domain_id}/subscriptions/{subscription_id}/activate"),
            Some(json!({})),
            subscription_not_found_message(&domain_id, &subscription_id),
            NOT_FOUND,
        ),
        (
            "DeactivateSubscription",
            "POST",
            format!("/domains/{domain_id}/subscriptions/{subscription_id}/deactivate"),
            Some(json!({})),
            subscription_not_found_message(&domain_id, &subscription_id),
            NOT_FOUND,
        ),
        (
            "StartPatientInsightsJob",
            "POST",
            format!("/domain/{domain_id}/patient-insights-job"),
            Some(start_job_body()),
            DOMAIN_NOT_AUTHORIZED_MESSAGE.to_string(),
            NOT_AUTHORIZED,
        ),
        (
            "GetPatientInsightsJob",
            "GET",
            format!("/domain/{domain_id}/patient-insights-job/{job_id}"),
            None,
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
        (
            "GetMedicalScribeListeningSession",
            "GET",
            format!(
                "/medical-scribe-stream/domain/{domain_id}/subscription/{subscription_id}\
                 /session/{SESSION_ID}"
            ),
            None,
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
        (
            "ListTagsForResource (domain ARN)",
            "GET",
            tags_path(&domain_arn),
            None,
            resource_not_found_message(&domain_arn),
            NOT_FOUND,
        ),
        (
            "ListTagsForResource (subscription ARN)",
            "GET",
            tags_path(&subscription_arn),
            None,
            resource_not_found_message(&subscription_arn),
            NOT_FOUND,
        ),
        (
            "TagResource (domain ARN)",
            "POST",
            tags_path(&domain_arn),
            Some(json!({ "tags": { "env": "crossed" } })),
            resource_not_found_message(&domain_arn),
            NOT_FOUND,
        ),
        (
            "TagResource (subscription ARN)",
            "POST",
            tags_path(&subscription_arn),
            Some(json!({ "tags": { "env": "crossed" } })),
            resource_not_found_message(&subscription_arn),
            NOT_FOUND,
        ),
        (
            "DeleteDomain",
            "DELETE",
            format!("/domain/{domain_id}"),
            None,
            domain_not_found_message(&domain_id),
            NOT_FOUND,
        ),
    ];

    for other in REGIONS.iter().filter(|region| **region != home) {
        for (operation, method, path, body, detail, refusal) in &rows {
            let case = format!("{operation} from {other}");
            let (status, answered) = emulator
                .send_in(other, method, path, body.as_ref())
                .await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                *refusal,
                "case {case:?}: a resource of another Region must answer {refusal:?}, \
                 got {status} {answered}"
            );
            assert_eq!(
                answered.get("Message").and_then(Value::as_str),
                Some(detail.as_str()),
                "case {case:?}: expected the detail text {detail:?}, got {answered}"
            );
            assert!(
                *refusal == NOT_FOUND || !answered.to_string().contains(domain_id.as_str()),
                "case {case:?}: an authorization refusal must not name the requested \
                 domain, got {answered}"
            );
        }
    }

    // Nothing above reached the store: the domain, its subscription, its tag set and its
    // job are exactly as the home Region left them.
    let (status, domain) = emulator
        .send_in(home, "GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain at home got {domain}");
    assert_eq!(
        domain.get("status").and_then(Value::as_str),
        Some("ACTIVE"),
        "the domain must still be ACTIVE after the cross-Region calls, got {domain}"
    );
    assert_eq!(
        domain.get("tags"),
        Some(&tags),
        "the tag set must be untouched by a cross-Region TagResource, got {domain}"
    );
    let (status, read_back) = emulator
        .send_in(
            home,
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "GetSubscription at home got {read_back}");
    assert_eq!(
        read_back
            .get("subscription")
            .and_then(|subscription| subscription.get("status"))
            .and_then(Value::as_str),
        Some("ACTIVE"),
        "the subscription must still be ACTIVE after the cross-Region calls, got {read_back}"
    );
    let (status, read_back) = emulator
        .send_in(
            home,
            "GET",
            &format!("/domain/{domain_id}/patient-insights-job/{job_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "GetPatientInsightsJob at home got {read_back}"
    );
    let (status, listed) = emulator
        .send_in(home, "GET", &tags_path(&domain_arn), None)
        .await;
    assert_eq!(status, 200, "ListTagsForResource at home got {listed}");
    assert_eq!(
        listed.get("tags"),
        Some(&tags),
        "the home Region must still report the original tag set, got {listed}"
    );
}

#[tokio::test]
async fn the_domain_cap_is_counted_per_region() {
    let emulator = Emulator::new(REGIONS[0]);
    let account = engine::emulator_account_id().to_string();
    // The text the cap refusal carries, as `CreateDomain` words it.
    let refusal = format!(
        "Account {account} has reached the maximum limit of {MAX_DOMAINS_PER_ACCOUNT} domains."
    );

    // Each Region in turn is filled to the cap and then refused, while the NEXT Region —
    // which the filled ones say nothing about — still accepts its first create. The rows
    // therefore differ along how much of the account's cross-Region total is already
    // spent: by the last Region the account holds several times the cap in all.
    for (index, region) in REGIONS.iter().enumerate() {
        for ordinal in 0..MAX_DOMAINS_PER_ACCOUNT {
            let created = emulator
                .create_domain_in(region, &json!({ "name": format!("clinic-{ordinal}") }))
                .await;
            assert_eq!(
                created.get("status").and_then(Value::as_str),
                Some("ACTIVE"),
                "case {region:?} #{ordinal}: a create inside the cap must succeed, got {created}"
            );
        }
        let (status, refused) = emulator
            .send_in(
                region,
                "POST",
                "/domain",
                Some(&json!({ "name": "clinic-past-the-cap" })),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {region:?}: the create past this Region's cap must be refused, got \
             {status} {refused}"
        );
        assert_eq!(
            refused.get("Message").and_then(Value::as_str),
            Some(refusal.as_str()),
            "case {region:?}: the refusal must name the account and the cap, got {refused}"
        );
        // The filled Region holds the cap; every Region after it still holds nothing.
        for later in REGIONS.iter().skip(index + 1) {
            let listed = listed_domain_ids(&emulator, later, "?maxResults=100").await;
            assert!(
                listed.is_empty(),
                "case {later:?}: a Region filled elsewhere must still be empty, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_store_holds_at_most_the_absolute_domain_ceiling() {
    // The Region a record is filed under is read off the request, so this table walks
    // MORE distinct Region values than the store files records under — and none of them
    // is a Region AWS operates, because nothing stops a caller signing for one. The
    // per-Region quota is spent in each, and the store still stops at its own ceiling.
    let emulator = Emulator::new(REGIONS[0]);
    let mut accepted = 0usize;

    for index in 0..(MAX_DOMAIN_REGIONS + 4) {
        let region = format!("r{index}-emulator-1");
        for ordinal in 0..MAX_DOMAINS_PER_ACCOUNT {
            let case = format!("{region} #{ordinal}");
            let (status, body) = emulator
                .send_in(
                    &region,
                    "POST",
                    "/domain",
                    Some(&json!({ "name": format!("clinic-{ordinal}") })),
                )
                .await;
            if accepted < MAX_DOMAINS_HELD {
                assert_eq!(
                    status, 201,
                    "case {case:?}: a create inside the ceiling must succeed, got {body}"
                );
                accepted += 1;
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (402, "ServiceQuotaExceededException"),
                    "case {case:?}: a create past the ceiling must be refused, got \
                     {status} {body}"
                );
            }
        }
    }

    assert_eq!(
        accepted, MAX_DOMAINS_HELD,
        "the store must accept exactly its ceiling of domains, got {accepted}"
    );
}

#[tokio::test]
async fn a_domain_belongs_to_the_region_its_arn_names() {
    // The scope and the ARN come from ONE reading of the request's credential scope, so a
    // request that names no usable region mints its domain in the fallback Region AND is
    // served from there — the two can never disagree.
    let emulator = Emulator::new(FALLBACK_REGION);
    let created = emulator
        .create_domain_in(UNUSABLE_REGION_SCOPE, &json!({ "name": "clinic-fallback" }))
        .await;
    let domain_id = string_at(&created, "domainId");
    let arn = string_at(&created, "arn");
    assert_eq!(
        arn_region(&arn),
        FALLBACK_REGION,
        "an unusable region scope must mint in the fallback Region, got {arn:?}"
    );

    let listed = listed_domain_ids(&emulator, FALLBACK_REGION, "?maxResults=100").await;
    assert!(
        listed.contains(&domain_id),
        "the fallback Region must list {domain_id}, got {listed:?}"
    );
    let (status, body) = emulator
        .send_in(FALLBACK_REGION, "GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(
        status, 200,
        "the fallback Region must resolve {domain_id}, got {body}"
    );

    for other in REGIONS.iter().filter(|region| **region != FALLBACK_REGION) {
        let listed = listed_domain_ids(&emulator, other, "?maxResults=100").await;
        assert!(
            listed.is_empty(),
            "case {other:?}: a Region that created nothing must list nothing, got {listed:?}"
        );
        let (status, body) = emulator
            .send_in(other, "GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            NOT_FOUND,
            "case {other:?}: a domain of the fallback Region must not resolve, got \
             {status} {body}"
        );
    }
}
