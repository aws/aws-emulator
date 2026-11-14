// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetHealthCheckStatus`.
//!
//! The rule under test: the operation reports one `HealthCheckObservation` for
//! each Route 53 health checker that is checking the health check's endpoint,
//! read out of the health check the account holds — the checkers are the
//! health-checking Regions the configuration names (all the modelled ones when
//! it names none), each reporting from an address inside a block
//! `GetCheckerIpRanges` publishes, with the description of the LATEST probe of
//! that endpoint and the instant of the last probe on its `RequestInterval`
//! cadence. A health
//! check nothing is checking — one whose `Type` probes no endpoint, or whose
//! checks are disabled — has no checker reporting on it, and an id that names no
//! health check of the account is refused rather than answered.
//!
//! Every endpoint these tables configure is one this emulator serves nothing on,
//! so the probe of it reaches nothing and that is what the checkers report; the
//! probe of an endpoint the emulator DOES serve, and the `SearchString` predicate
//! applied to the body it answers with, is driven over two services in
//! `core/router/tests/all/route53_health_check_string_match.rs`.
//!
//! So the tables below drive the status over a SET of health checks rather than
//! one: every `HealthCheckType` in the model's own value list, several
//! Region sets of different sizes and orders, the `Disabled` setting both as
//! created and as an update leaves it, and ids that name nothing. The operation
//! is a report, so each table also asserts that reading it twice says the same
//! thing and leaves the account as it was.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use std::net::Ipv4Addr;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::HEALTH_CHECK_REGION_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::endpoint_probe::STATUS_CONNECTION_FAILED;
use service_com_amazonaws_route53_awsdnsv20130401::state::HEALTH_CHECK_TYPE_CALCULATED;

use crate::checker_ip_ranges::reported_blocks;
use crate::health_check_updates::update_health_check;
use crate::health_checks::{
    child_for, create_health_check, health_check_count, health_check_id, CATALOG,
};
use crate::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_ok, router, RecordRow, Reply,
};

/// One observation as the wire carries it.
///
/// `pub(crate)` so the sibling suite that asserts the OTHER reader of a health
/// check's observations reads them out of the wire the same way rather than
/// growing a second copy of this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) region: String,
    pub(crate) ip_address: String,
    pub(crate) status: String,
    pub(crate) checked_time: String,
}

/// Send one `GetHealthCheckStatus` request.
///
/// `pub(crate)` so a sibling suite reads the status through the SAME request.
pub(crate) async fn get_status(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}/status"),
        "",
    )
    .await
}

/// The observations `GetHealthCheckStatus` reports for `id`, in wire order.
///
/// A 501 here means the operation is still a scaffold, which is exactly what the
/// message says, so a regression to the stub reads as one.
async fn observations(router: &MultiServiceRouter, id: &str, case: &str) -> Vec<Observation> {
    let reply = get_status(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckStatus (501 = still scaffolded), \
         got {} with body {}",
        reply.status, reply.body
    );
    observations_in(&reply, case)
}

/// The observations one reply carries, in wire order.
///
/// `pub(crate)` because `GetHealthCheckStatus` and
/// `GetHealthCheckLastFailureReason` answer the SAME modelled list, so both
/// suites read it out of the body through this one reader.
pub(crate) fn observations_in(reply: &Reply, case: &str) -> Vec<Observation> {
    let regions = reply.all("HealthCheckObservation/Region");
    let addresses = reply.all("HealthCheckObservation/IPAddress");
    let statuses = reply.all("StatusReport/Status");
    let times = reply.all("StatusReport/CheckedTime");
    assert!(
        regions.len() == addresses.len()
            && regions.len() == statuses.len()
            && regions.len() == times.len(),
        "case {case:?}: every observation must carry a Region, an IPAddress and a StatusReport, \
         got {} region(s), {} address(es), {} status(es) and {} time(s); body {}",
        regions.len(),
        addresses.len(),
        statuses.len(),
        times.len(),
        reply.body
    );
    (0..regions.len())
        .map(|index| Observation {
            region: regions[index].to_string(),
            ip_address: addresses[index].to_string(),
            status: statuses[index].to_string(),
            checked_time: times[index].to_string(),
        })
        .collect()
}

/// The status a checker reports for a health check whose configured endpoint is
/// one THIS emulator serves nothing on: no checker of it can connect, so what
/// every one of them reports is that transport failure. Which criterion the
/// `Type` sets is immaterial — there was no connection to apply it to — and the
/// suite that drives a check against an endpoint the emulator DOES serve is
/// `core/router/tests/all/route53_health_check_string_match.rs`.
fn expected_status(_kind: &str) -> &'static str {
    STATUS_CONNECTION_FAILED
}

/// `(network address, prefix length)` of an `a.b.c.d/nn` block, or `None` when
/// the text is not a well-formed IPv4 CIDR block.
fn parse_cidr(block: &str) -> Option<(u32, u8)> {
    let (address, prefix) = block.split_once('/')?;
    let address = u32::from(address.parse::<Ipv4Addr>().ok()?);
    let prefix: u8 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    Some((address, prefix))
}

/// Whether `address` is inside the CIDR `block`.
fn block_holds(block: &str, address: &str) -> bool {
    let Some((network, prefix)) = parse_cidr(block) else {
        return false;
    };
    let Ok(address) = address.parse::<Ipv4Addr>() else {
        return false;
    };
    // The bits a /nn fixes, as a mask. `checked_shl` keeps a /0 (no fixed bits)
    // from shifting by the full width of the type, which panics.
    let mask = u32::MAX
        .checked_shl(32 - u32::from(prefix))
        .unwrap_or(u32::MAX);
    if prefix == 0 {
        return true;
    }
    u32::from(address) & mask == network & mask
}

/// A sortable key for an ISO-8601 UTC timestamp (`2017-03-27T17:48:16.751Z`):
/// its date and time digits as one number, and its milliseconds.
///
/// Written here rather than reached from the crate under test, and on the TEXT
/// the wire carries, so a timestamp that is not a UTC instant of that form fails
/// here instead of being compared as something else.
fn instant_key(text: &str) -> Option<(u64, u32)> {
    let body = text.strip_suffix('Z')?;
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    // `YYYY-MM-DDTHH:MM:SS` carries exactly fourteen digits; anything else is
    // not that form.
    let digits: String = whole.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 14 || whole.len() != 19 {
        return None;
    }
    let millis: u32 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<3}").get(..3)?.parse().ok()?
    };
    Some((digits.parse().ok()?, millis))
}

/// An instant read off the service's own clock, through an operation that
/// reports one: the `SubmittedAt` of a change submitted into `zone`.
///
/// Used to bound the `CheckedTime` a status reports from both sides, so the
/// assertion is against the service's clock rather than the test's.
async fn service_instant(
    router: &MultiServiceRouter,
    zone: &str,
    record: &'static str,
) -> (u64, u32) {
    let row = RecordRow {
        name: record,
        rr_type: "A",
        ttl: 300,
        values: &["192.0.2.10"],
    };
    let reply = call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone}/rrset"),
        &change_batch_body(&change_element("CREATE", &row), None),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ChangeResourceRecordSets, got {} with body {}",
        reply.status, reply.body
    );
    let submitted = reply.first("ChangeInfo/SubmittedAt").unwrap_or_else(|| {
        panic!(
            "a change must report its SubmittedAt; body {}",
            reply.body
        )
    });
    instant_key(submitted).unwrap_or_else(|| {
        panic!("SubmittedAt must be an ISO-8601 UTC instant, got {submitted:?}")
    })
}

#[tokio::test]
async fn one_checker_per_health_checking_region_reports_on_a_health_check_that_probes_an_endpoint() {
    // One row per modelled `HealthCheckType` (the catalog the sibling suite
    // keeps pinned to the model's own value list), because what a health check
    // IS varies by type: four of them probe an endpoint for a checker to report
    // about, and the other four read a CloudWatch alarm, combine child health
    // checks or follow a routing control.
    for row in CATALOG {
        let router = router();
        let published = reported_blocks(&router).await;
        let child = child_for(&router, row).await;
        let created =
            create_health_check(&router, &format!("status-{}", row.kind), &row.config(&child)).await;
        let id = health_check_id(&created, row.kind);
        // A CALCULATED health check has no endpoint and no checkers, so this
        // reader refuses it rather than answering with a list; the refusal's
        // status and wording are asserted in `health_check_calculated_reads`.
        if row.kind == HEALTH_CHECK_TYPE_CALCULATED {
            let reply = get_status(&router, &id).await;
            assert_eq!(
                reply.status, 400,
                "case {:?}: expected 400 from GetHealthCheckStatus, got {} with body {}",
                row.kind, reply.status, reply.body
            );
            continue;
        }
        let reported = observations(&router, &id, row.kind).await;

        if !row.endpoint_probe {
            assert!(
                reported.is_empty(),
                "case {:?}: a health check that probes no endpoint has no checker reporting \
                 on it, got {reported:?}",
                row.kind
            );
            continue;
        }

        let regions: Vec<&str> = reported.iter().map(|o| o.region.as_str()).collect();
        assert_eq!(
            regions, HEALTH_CHECK_REGION_VALUES,
            "case {:?}: a health check that names no Regions is checked from every modelled \
             health-checking Region, got {regions:?}",
            row.kind
        );
        for observation in &reported {
            assert_eq!(
                observation.status,
                expected_status(row.kind),
                "case {:?}: expected the description of a probe that reached nothing, got {:?}",
                row.kind,
                observation.status
            );
            assert!(
                observation.status.starts_with("Success: ")
                    || observation.status.starts_with("Failure: "),
                "case {:?}: a checker's status reads as a Success/Failure description, got {:?}",
                row.kind,
                observation.status
            );
            assert!(
                published
                    .iter()
                    .any(|block| block_holds(block, &observation.ip_address)),
                "case {:?}: the checker in {:?} must report from an address inside a published \
                 health-checker block, got {:?} against {published:?}",
                row.kind,
                observation.region,
                observation.ip_address
            );
            assert!(
                instant_key(&observation.checked_time).is_some(),
                "case {:?}: CheckedTime must be an ISO-8601 UTC instant, got {:?}",
                row.kind,
                observation.checked_time
            );
        }
        // Every checker reports the same probe round, so one health check has
        // one `CheckedTime` across its observations.
        let times: Vec<&str> = reported
            .iter()
            .map(|o| o.checked_time.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            times.len(),
            1,
            "case {:?}: the checkers report one probe round, got CheckedTime(s) {times:?}",
            row.kind
        );
    }
}

#[tokio::test]
async fn the_regions_the_configuration_names_are_exactly_the_checkers_that_report() {
    // The rows differ along the SIZE and the ORDER of the Region set, and one
    // names a Region twice: the reported checkers are the set the configuration
    // names, each once, in the one order the modelled `HealthCheckRegion` value
    // list gives them, whatever order the request spelled them in.
    struct Row {
        case: &'static str,
        regions: &'static [&'static str],
        expected: &'static [&'static str],
    }
    // `Regions` is modelled `len 3..64`, so three is the smallest set a request
    // may carry (`smithy_query(query="shape",shape="HealthCheckRegionList")`).
    let rows = &[
        Row {
            case: "the smallest set the model allows",
            regions: &["us-east-1", "us-west-2", "eu-west-1"],
            expected: &["eu-west-1", "us-east-1", "us-west-2"],
        },
        Row {
            case: "four Regions, spelled in reverse order",
            regions: &["sa-east-1", "ap-southeast-2", "ap-southeast-1", "ap-northeast-1"],
            expected: &["ap-northeast-1", "ap-southeast-1", "ap-southeast-2", "sa-east-1"],
        },
        Row {
            case: "every modelled Region, spelled out",
            regions: HEALTH_CHECK_REGION_VALUES,
            expected: HEALTH_CHECK_REGION_VALUES,
        },
        Row {
            case: "a Region named twice",
            regions: &["us-west-1", "us-west-1", "eu-west-1", "us-east-1"],
            expected: &["eu-west-1", "us-east-1", "us-west-1"],
        },
    ];

    for row in rows {
        let router = router();
        let regions: String = row
            .regions
            .iter()
            .map(|region| format!("<Region>{region}</Region>"))
            .collect();
        let created = create_health_check(
            &router,
            &format!("status-regions-{}", row.regions.len()),
            &format!(
                "<Type>HTTP</Type><IPAddress>192.0.2.80</IPAddress><Port>80</Port>\
                 <Regions>{regions}</Regions>"
            ),
        )
        .await;
        let id = health_check_id(&created, row.case);
        let reported = observations(&router, &id, row.case).await;
        let got: Vec<&str> = reported.iter().map(|o| o.region.as_str()).collect();
        assert_eq!(
            got, row.expected,
            "case {:?}: expected the checkers {:?}, got {got:?}",
            row.case, row.expected
        );
        // Two checkers in different Regions are two different checkers, so no
        // address is reported twice.
        let addresses: std::collections::BTreeSet<&str> =
            reported.iter().map(|o| o.ip_address.as_str()).collect();
        assert_eq!(
            addresses.len(),
            reported.len(),
            "case {:?}: each reporting checker has its own address, got {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_health_check_whose_checks_are_disabled_has_no_checker_reporting_on_it() {
    // Disabled stops Route 53 submitting requests to the endpoint, so nothing is
    // reporting about it. The rows span the types that probe an endpoint, and
    // each is then re-enabled through UpdateHealthCheck, so the setting is
    // asserted both as a create persisted it and as an update left it.
    for row in CATALOG.iter().filter(|row| row.endpoint_probe) {
        let router = router();
        let created = create_health_check(
            &router,
            &format!("status-disabled-{}", row.kind),
            &format!("{}<Disabled>true</Disabled>", row.config("")),
        )
        .await;
        let id = health_check_id(&created, row.kind);
        let disabled = observations(&router, &id, row.kind).await;
        assert!(
            disabled.is_empty(),
            "case {:?}: a disabled health check has no checker reporting on it, got {disabled:?}",
            row.kind
        );

        let updated = update_health_check(&router, &id, "<Disabled>false</Disabled>").await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateHealthCheck, got {} with body {}",
            row.kind, updated.status, updated.body
        );
        let enabled = observations(&router, &id, row.kind).await;
        assert_eq!(
            enabled.len(),
            HEALTH_CHECK_REGION_VALUES.len(),
            "case {:?}: re-enabling the checks brings every checker back, got {enabled:?}",
            row.kind
        );

        let disabled_again = update_health_check(&router, &id, "<Disabled>true</Disabled>").await;
        assert_eq!(
            disabled_again.status, 200,
            "case {:?}: expected 200 from the second UpdateHealthCheck, got {} with body {}",
            row.kind, disabled_again.status, disabled_again.body
        );
        let silent_again = observations(&router, &id, row.kind).await;
        assert!(
            silent_again.is_empty(),
            "case {:?}: disabling the checks again silences every checker, got {silent_again:?}",
            row.kind
        );
    }
}

#[tokio::test]
async fn reading_the_status_reports_the_same_observations_and_changes_nothing() {
    // The operation is a report: the rows are repeated reads, with other calls
    // in between, and every one of them has to say what the first said and leave
    // the account's health checks exactly as they were.
    let router = router();
    let created = create_health_check(
        &router,
        "status-stable",
        "<Type>HTTPS_STR_MATCH</Type><FullyQualifiedDomainName>stable.example.com\
         </FullyQualifiedDomainName><SearchString>SERVING</SearchString>\
         <RequestInterval>30</RequestInterval>",
    )
    .await;
    let id = health_check_id(&created, "status-stable");
    let first = observations(&router, &id, "the first read").await;
    assert!(
        !first.is_empty(),
        "a health check that probes an endpoint has checkers reporting on it; got none"
    );
    let population = health_check_count(&router, "after the first read").await;

    for attempt in 1..=3 {
        let case = format!("read {attempt}");
        let again = observations(&router, &id, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated read must report the same observations, got {again:?}"
        );
        let counted = health_check_count(&router, &case).await;
        assert_eq!(
            counted, population,
            "case {case:?}: reading the status must not create or destroy a health check, \
             GetHealthCheckCount reports {counted}"
        );
    }
}

#[tokio::test]
async fn the_checked_time_is_an_instant_inside_the_health_check_s_lifetime() {
    // The probe a read reports is one the clock has already come round for, so
    // its CheckedTime lies between the moment the health check was created and
    // the moment it is read — both taken from the service's own clock, through
    // an operation that reports one.
    let router = router();
    let zone = create_zone_ok(&router, "checked-time.example.com", "status-checked-time").await;
    let before = service_instant(&router, &zone, "before.checked-time.example.com").await;
    let created = create_health_check(
        &router,
        "status-checked-time",
        "<Type>TCP</Type><IPAddress>192.0.2.90</IPAddress><Port>443</Port>\
         <RequestInterval>30</RequestInterval>",
    )
    .await;
    let id = health_check_id(&created, "status-checked-time");
    let reported = observations(&router, &id, "the checked time").await;
    let after = service_instant(&router, &zone, "after.checked-time.example.com").await;

    assert!(
        !reported.is_empty(),
        "a TCP health check has checkers reporting on it; got none"
    );
    for observation in &reported {
        let checked = instant_key(&observation.checked_time).unwrap_or_else(|| {
            panic!(
                "case {:?}: CheckedTime must be an ISO-8601 UTC instant, got {:?}",
                observation.region, observation.checked_time
            )
        });
        assert!(
            checked >= before && checked <= after,
            "case {:?}: expected a CheckedTime between {before:?} and {after:?}, got {:?} \
             ({checked:?})",
            observation.region,
            observation.checked_time
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_health_check_is_refused() {
    // The rows differ along WHY the id names nothing: never created, created on
    // another store, and a well-formed id of another entity kind. A refusal
    // rather than an empty answer is what tells a client the health check is
    // gone.
    let router = router();
    let created = create_health_check(
        &router,
        "status-refusal",
        "<Type>HTTP</Type><IPAddress>192.0.2.81</IPAddress><Port>80</Port>",
    )
    .await;
    let known = health_check_id(&created, "status-refusal");

    let other_store = crate::hosted_zone_records::router();
    let elsewhere = create_health_check(
        &other_store,
        "status-refusal-elsewhere",
        "<Type>HTTP</Type><IPAddress>192.0.2.82</IPAddress><Port>80</Port>",
    )
    .await;
    let on_another_store = health_check_id(&elsewhere, "status-refusal-elsewhere");

    for (case, id) in [
        ("an id no create ever minted", "00000000-1111-2222-3333-444444444444"),
        ("a health check of another store", on_another_store.as_str()),
        ("a hosted-zone id", "Z1D633PJN98FT9"),
        ("a single character", "x"),
    ] {
        assert_ne!(
            id, known,
            "case {case:?}: the row must not name the health check this store holds"
        );
        let reply = get_status(&router, id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404 from GetHealthCheckStatus, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHealthCheck",
            "case {case:?}: expected the NoSuchHealthCheck code, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
    }

    // And the health check the store DOES hold is still readable, so the
    // refusals above changed nothing.
    let reported = observations(&router, &known, "the health check that exists").await;
    assert_eq!(
        reported.len(),
        HEALTH_CHECK_REGION_VALUES.len(),
        "the health check the store holds must still be reported on, got {reported:?}"
    );
}
