// SPDX-License-Identifier: CC0-1.0

//! WHERE EC2 answers the modelled `DryRun` member, driven over the wire the service actually
//! speaks (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rule under test: `DryRun` "checks whether you have the required permissions for the
//! action, WITHOUT ACTUALLY MAKING THE REQUEST, and provides an error response. If you have
//! the required permissions, the error response is DryRunOperation. Otherwise, it is
//! UnauthorizedOperation" (the member's own doc, on every input shape that models it). So the
//! flag is a question about the CALLER, answered ahead of performing the request, and the
//! permitted side of it is HTTP 412 `DryRunOperation` with one fixed sentence — never a report
//! on the work the operation would have done, and never the 501 that says nobody has written
//! the operation yet.
//!
//! Two tables, one per consequence:
//!
//! 1. an operation this emulator has NOT written answers the flag too. Its 501 is the signal
//!    that no implementation read the request and nothing was performed, which is exactly the
//!    case the flag's own answer covers, so the verdict stands in place of that 501; without
//!    the flag the same request still answers 501, which is the honest "not written yet". The
//!    exception is an operation this emulator HAS written that answers the same 501 for a
//!    MEMBER it holds no state for: there the 501 is about the REQUEST, the flag leaves it
//!    alone, and the set of such operations is read off this crate's own sources rather than
//!    trusted to a hand-kept list.
//! 2. WHERE the flag sits among an implemented operation's own checks is a fact of that
//!    operation, recorded per operation: some answer the flag before they read the request's
//!    values at all, some after the values' form, and one resolves the single resource the
//!    call is scoped to first. The rows span those three positions and, per operation, the
//!    kinds of input the position generalises over (a value that is not an id of its kind, a
//!    well-formed id naming nothing, a value outside a published range).
//!
//! No row is the specification: each is one input along what the rule generalises over. The
//! codes and the sentences asserted here are written out as the recorded responses give them,
//! never read back out of the implementation.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::fpga_image_tasks_and_volume_io::{seed_fpga_image, seed_image_usage_report};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// EC2's answer to an authorized dry run, the status it carries it on, and its whole
/// sentence — which names nothing of the request.
const DRY_RUN_CODE: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// What an operation nobody has written answers, and the status it carries.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The status every refusal of a REQUEST in the tables below carries.
const REFUSAL_STATUS: u16 = 400;

/// A well-formed Capacity Reservation id the account holds nothing under.
const ABSENT_RESERVATION: &str = "cr-0fedcba987654321f";

/// A well-formed AFI id the account holds no FPGA image for.
const ABSENT_FPGA_IMAGE: &str = "afi-0fedcba987654321f";

/// A well-formed AMI usage report id the account holds no report for.
const ABSENT_IMAGE_USAGE_REPORT: &str = "imgusgrpt-0fedcba987654321f";

/// A well-formed VPC endpoint service id the account holds no service for.
const ABSENT_ENDPOINT_SERVICE: &str = "vpce-svc-0fedcba987654321f";

/// A well-formed VPC endpoint id the account holds no endpoint for.
const ABSENT_ENDPOINT: &str = "vpce-0fedcba987654321f";

/// A well-formed transit gateway id the account holds no gateway for.
const ABSENT_TRANSIT_GATEWAY: &str = "tgw-0fedcba987654321f";

/// An Elastic IP address outside the pool this emulator allocates from, so the account holds
/// no allocation for it (`203.0.113.0/24` is the documentation range RFC 5737 reserves).
const UNHELD_ADDRESS: &str = "203.0.113.9";

/// A twelve-digit account id, the form `AccountID` publishes (`length 12..12`, pattern
/// `^[0-9]{12}$`), for the billing-owner calls that require one.
const ANOTHER_ACCOUNT: &str = "123456789012";

/// One row of either table: the case, the action, and the request's members beside the flag.
type Row = (&'static str, &'static str, Vec<Param>);

/// One row of the ordering table: the case, the action, the members beside the flag, and the
/// status and code the call is answered with WHEN the flag is set.
type OrderedRow = (&'static str, &'static str, Vec<Param>, u16, &'static str);

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The rows of operations this emulator has not written: each carries every member its input
/// shape requires, so the boundary validator admits the request and the call reaches the
/// scaffold rather than being refused for its form.
fn unwritten_operation_rows() -> Vec<Row> {
    vec![
        (
            "a billing assignment to another account",
            "AssociateCapacityReservationBillingOwner",
            vec![
                param("CapacityReservationId", ABSENT_RESERVATION),
                param("UnusedReservationBillingOwnerId", ANOTHER_ACCOUNT),
            ],
        ),
        (
            "taking a billing assignment back",
            "DisassociateCapacityReservationBillingOwner",
            vec![
                param("CapacityReservationId", ABSENT_RESERVATION),
                param("UnusedReservationBillingOwnerId", ANOTHER_ACCOUNT),
            ],
        ),
        (
            "rejecting a billing assignment",
            "RejectCapacityReservationBillingOwnership",
            vec![param("CapacityReservationId", ABSENT_RESERVATION)],
        ),
        (
            "reading Capacity Manager metric data",
            "GetCapacityManagerMetricData",
            vec![
                // One member of the published `Metric` catalog
                // (`smithy_query(query="shape",shape="Metric")`), the shortest period the
                // model admits (`Period`, range 3600..) and a window of one day.
                param("MetricName.1", "reservation-total-count"),
                param("StartTime", "2026-01-01T00:00:00Z"),
                param("EndTime", "2026-01-02T00:00:00Z"),
                param("Period", "3600"),
            ],
        ),
        (
            "unlocking a snapshot",
            "UnlockSnapshot",
            vec![param("SnapshotId", "snap-0fedcba987654321f")],
        ),
    ]
}

/// RULE 1: a dry run of an operation this emulator has not written is answered the flag's own
/// verdict, not the scaffold's 501.
#[tokio::test]
async fn an_unwritten_operation_answers_the_dry_run_verdict() {
    let (router, engine) = emulator();
    let stored_bytes = || engine.state::<Ec2State>().lock().stored_bytes();
    let before = stored_bytes();

    for (case, action, members) in unwritten_operation_rows() {
        let mut params = members.clone();
        params.push(param("DryRun", "true"));
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_CODE,
            "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_MESSAGE,
            "case {case:?}: expected EC2's own dry-run wording, got body {body}"
        );
    }

    // A dry run performs nothing, and an operation nobody has written performs nothing
    // either, so the store holds exactly what it held before the first call.
    let after = stored_bytes();
    assert_eq!(
        after, before,
        "no dry run of an unwritten operation may store anything: the store held {before} \
         bytes and now holds {after}"
    );
}

/// RULE 1, the other half: WITHOUT the flag the same request still answers the scaffold's 501,
/// so the verdict above speaks for the flag alone and the 501 still says "not written yet".
#[tokio::test]
async fn without_the_flag_an_unwritten_operation_still_answers_501() {
    let (router, _engine) = emulator();

    for (case, action, members) in unwritten_operation_rows() {
        // Rows differing along how the flag is left off: absent altogether, and explicitly
        // off — `DryRun=false` is not a dry run, it is an ordinary call.
        for (flag, flag_case) in [(None, "no flag"), (Some("false"), "DryRun=false")] {
            let mut params = members.clone();
            if let Some(value) = flag {
                params.push(param("DryRun", value));
            }
            let (status, body) = call(&router, REGION, action, &params).await;
            assert_eq!(
                status, NOT_IMPLEMENTED_STATUS,
                "case {case:?} with {flag_case}: expected HTTP {NOT_IMPLEMENTED_STATUS}, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_IMPLEMENTED_CODE,
                "case {case:?} with {flag_case}: expected {NOT_IMPLEMENTED_CODE}, \
                 got body {body}"
            );
        }
    }
}

/// RULE 2: the recorded position of the flag among each operation's own checks — the three
/// positions, each with the inputs its position generalises over.
#[tokio::test]
async fn the_flag_is_answered_where_each_operation_is_recorded_answering_it() {
    // (case, action, the members beside the flag, the status and code a DRY RUN is answered
    // with)
    let rows: Vec<OrderedRow> = vec![
        // Answered before the request's own values are read at all: the recorded contract
        // answers the verdict even for a value that is not an id of its kind, which is the
        // documented idiom for a pure permission probe.
        (
            "an AFI delete naming something that is not an AFI id",
            "DeleteFpgaImage",
            vec![param("FpgaImageId", "afi-bogus")],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "an AFI delete naming an id the account holds no AFI for",
            "DeleteFpgaImage",
            vec![param("FpgaImageId", ABSENT_FPGA_IMAGE)],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "a report delete naming something that is not a report id",
            "DeleteImageUsageReport",
            vec![param("ReportId", "imgusgrpt-bogus")],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "a report delete naming an id the account holds no report for",
            "DeleteImageUsageReport",
            vec![param("ReportId", ABSENT_IMAGE_USAGE_REPORT)],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        // Answered once the request's own members have been read and before anything is
        // resolved: a resource the account does not hold is still the verdict, a value
        // outside the member's own form or range is not.
        (
            "an extension search naming a reservation the account does not hold",
            "DescribeCapacityBlockExtensionOfferings",
            vec![
                param("CapacityReservationId", ABSENT_RESERVATION),
                param("CapacityBlockExtensionDurationHours", "24"),
            ],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "an extension search asking for no hours at all",
            "DescribeCapacityBlockExtensionOfferings",
            vec![
                param("CapacityReservationId", ABSENT_RESERVATION),
                param("CapacityBlockExtensionDurationHours", "0"),
            ],
            REFUSAL_STATUS,
            "InvalidParameterValue",
        ),
        (
            "an acceptance of a transfer on an address the account does not hold",
            "AcceptAddressTransfer",
            vec![param("Address", UNHELD_ADDRESS)],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "an acceptance naming a value that is not an address at all",
            "AcceptAddressTransfer",
            vec![param("Address", "not-an-address")],
            REFUSAL_STATUS,
            "InvalidParameterValue",
        ),
        (
            "a billing acceptance naming an id this account was sent no request for",
            "AcceptCapacityReservationBillingOwnership",
            vec![param("CapacityReservationId", ABSENT_RESERVATION)],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        (
            "a billing acceptance naming a value EC2 could never have issued",
            "AcceptCapacityReservationBillingOwnership",
            vec![param("CapacityReservationId", "cr-00000000000000000")],
            REFUSAL_STATUS,
            "InvalidCapacityReservationId.Malformed",
        ),
        (
            "a transit gateway delete naming a gateway the account does not hold",
            "DeleteTransitGateway",
            vec![param("TransitGatewayId", ABSENT_TRANSIT_GATEWAY)],
            DRY_RUN_STATUS,
            DRY_RUN_CODE,
        ),
        // Answered only once the ONE service the call is scoped to has been resolved: a dry
        // run naming a service that does not exist is told so rather than that the request
        // would have succeeded.
        (
            "accepting connections to a service the account does not hold",
            "AcceptVpcEndpointConnections",
            vec![
                param("ServiceId", ABSENT_ENDPOINT_SERVICE),
                param("VpcEndpointId.1", ABSENT_ENDPOINT),
            ],
            REFUSAL_STATUS,
            "InvalidVpcEndpointServiceId.NotFound",
        ),
        (
            "accepting connections naming something that is not a service id",
            "AcceptVpcEndpointConnections",
            vec![
                param("ServiceId", "not-a-service-id"),
                param("VpcEndpointId.1", ABSENT_ENDPOINT),
            ],
            REFUSAL_STATUS,
            "InvalidVpcEndpointServiceId.Malformed",
        ),
    ];

    for (case, action, members, expected_status, expected_code) in rows {
        let (router, _engine) = emulator();
        let mut params = members.clone();
        params.push(param("DryRun", "true"));
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} under the flag, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code} under the flag, got body {body}"
        );
        if expected_code == DRY_RUN_CODE {
            assert_eq!(
                error_message(&body),
                DRY_RUN_MESSAGE,
                "case {case:?}: expected EC2's own dry-run wording, got body {body}"
            );
        }
    }
}

/// RULE 2, continued: a dry run of a request that WOULD have succeeded performs nothing — the
/// resources it names are still there afterwards, and the same call without the flag is the
/// one that removes them.
#[tokio::test]
async fn a_dry_run_of_a_request_that_would_have_succeeded_performs_nothing() {
    let (router, engine) = emulator();
    let (fpga_image_id, _) = seed_fpga_image(&engine, "the AFI a dry run must leave alone");
    let report_id = seed_image_usage_report(&engine, "ami-0123456789abcdef0");

    let held = |case: &str| {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let held = (
            guard.fpga_images.contains_key(&fpga_image_id),
            guard.image_usage_reports.contains_key(&report_id),
        );
        assert_eq!(
            held,
            (true, true),
            "case {case:?}: both seeded resources must still be stored, got {held:?}"
        );
    };
    held("before any dry run");

    // Rows differing along how many dry runs have already been answered: each asks about the
    // caller, so answering one neither removes anything nor changes the next answer.
    for attempt in 1..=3 {
        for (action, members) in [
            ("DeleteFpgaImage", vec![param("FpgaImageId", &fpga_image_id)]),
            ("DeleteImageUsageReport", vec![param("ReportId", &report_id)]),
        ] {
            let case = format!("{action} dry run {attempt}");
            let mut params = members.clone();
            params.push(param("DryRun", "true"));
            let (status, body) = call(&router, REGION, action, &params).await;
            assert_eq!(
                status, DRY_RUN_STATUS,
                "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                DRY_RUN_CODE,
                "case {case:?}: expected {DRY_RUN_CODE}, got body {body}"
            );
            held(&case);
        }
    }

    // And the same calls without the flag are the ones that remove the resources.
    for (action, members) in [
        ("DeleteFpgaImage", vec![param("FpgaImageId", &fpga_image_id)]),
        ("DeleteImageUsageReport", vec![param("ReportId", &report_id)]),
    ] {
        let case = format!("{action} for real");
        let (status, body) = call(&router, REGION, action, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert!(
        !guard.fpga_images.contains_key(&fpga_image_id)
            && !guard.image_usage_reports.contains_key(&report_id),
        "the calls made without the flag must have removed both resources"
    );
}

/// RULE 1, the exception the gate has to know about: an operation this emulator HAS written
/// can answer the same 501 for a MEMBER of the request it holds no state for, and there the
/// 501 is a statement about the request rather than about the operation — so the flag must NOT
/// turn it into "the request would have succeeded".
#[tokio::test]
async fn a_501_a_handler_answers_for_a_member_stays_a_501_under_the_flag() {
    // (case, action, the members beside the flag) — one row per such operation this suite can
    // drive from an empty account, each naming the member its handler holds no state for.
    let rows: Vec<Row> = vec![
        (
            "a route table associated with a public IPv4 pool",
            "AssociateRouteTable",
            vec![
                param("RouteTableId", "rtb-0fedcba987654321f"),
                param("SubnetId", "subnet-0fedcba987654321f"),
                param("PublicIpv4Pool", "ipv4pool-ec2-0fedcba987654321f"),
            ],
        ),
        (
            "a VPC created from an IPAM pool",
            "CreateVpc",
            vec![
                param("Ipv4IpamPoolId", "ipam-pool-0fedcba987654321f"),
                param("Ipv4NetmaskLength", "16"),
            ],
        ),
        (
            "a subnet whose CIDR comes from an IPAM pool",
            "CreateSubnet",
            vec![
                param("VpcId", "vpc-0fedcba987654321f"),
                param("Ipv4IpamPoolId", "ipam-pool-0fedcba987654321f"),
                param("Ipv4NetmaskLength", "24"),
            ],
        ),
    ];

    for (case, action, members) in rows {
        let (router, _engine) = emulator();
        let mut params = members.clone();
        params.push(param("DryRun", "true"));
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, NOT_IMPLEMENTED_STATUS,
            "case {case:?}: expected HTTP {NOT_IMPLEMENTED_STATUS} under the flag, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_IMPLEMENTED_CODE,
            "case {case:?}: expected {NOT_IMPLEMENTED_CODE} under the flag, got body {body}"
        );

        // The same request without the flag answers the same 501: the flag changed nothing
        // about work this emulator cannot perform.
        let (status, body) = call(&router, REGION, action, &members).await;
        assert_eq!(
            status, NOT_IMPLEMENTED_STATUS,
            "case {case:?}: expected HTTP {NOT_IMPLEMENTED_STATUS} without the flag, got \
             {status} with body {body}"
        );
    }
}

/// RULE 1, continued — the CENSUS the exception above rests on: the gate's list of operations
/// whose own handler can answer the scaffold's 501 is exactly the set of operation files that
/// carry no `SCAFFOLDED, NOT IMPLEMENTED` marker and still name `not_implemented`.
///
/// Read off this crate's own sources, the way the project's stub census is read, so a handler
/// that grows (or loses) such an answer fails this test instead of silently changing what a
/// dry run of it reads.
#[test]
fn the_gate_names_every_implemented_operation_that_can_answer_not_implemented() {
    let ops = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ops");
    let mut found: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&ops)
        .unwrap_or_else(|error| panic!("the operation sources must be readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("each operation source must be readable: {error}"))
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("the operation source {} must be readable: {error}", path.display())
        });
        // A stub answers the 501 for the whole operation, which is the case the gate speaks
        // for; an IMPLEMENTED file that still names the scaffold answers it for a member.
        if source.contains("//! SCAFFOLDED, NOT IMPLEMENTED:") || !source.contains("not_implemented(")
        {
            continue;
        }
        // The operation's own name, as its `impl Operation` declares it.
        let name = source
            .split("const NAME: &'static str = \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| {
                panic!("{} must declare its operation name", path.display())
            });
        found.push(name.to_string());
    }
    found.sort();
    let listed: Vec<String> = ec2::service::dry_run::OPERATIONS_WHOSE_HANDLER_ANSWERS_NOT_IMPLEMENTED
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        found, listed,
        "the gate's list must name exactly the implemented operations whose handler can \
         answer the scaffold's 501: the sources say {found:?}, the list says {listed:?}"
    );
}
