// SPDX-License-Identifier: CC0-1.0

//! WHICH requests an Application Load Balancer counts into `RequestCount`.
//!
//! The rule under test: **`RequestCount` is "the number of requests processed over IPv4 and
//! IPv6", and "this metric is only incremented for requests where the load balancer node was
//! able to choose a target" — so a request the load balancer answered ITSELF, with no target
//! chosen for it, is counted into neither the load balancer's own series nor any target group's,
//! while a request a target WAS chosen for is counted into both, whether that target then
//! answered or could not be reached at all.** The status such a request ended in is still
//! counted where the load-balancer-generated classes live, which is what makes the series
//! readable: a period in which every request was answered by the listener itself publishes the
//! `HTTPCode_ELB_*` counts and no request processed.
//!
//! The metric's name, its dimensions, its reporting criteria ("there is a non-zero value") and
//! the sentence above are Elastic Load Balancing's published CloudWatch metrics for the
//! Application Load Balancer family.
//!
//! The rows differ along what the rule generalises over — which end answered and why: a
//! `fixed-response` action, a `redirect` action, a `forward` whose target group holds no target
//! to send the request to, a `forward` whose chosen target cannot be connected to, and a
//! `forward` whose chosen target answers. The first three are answers the load balancer
//! produced with no target chosen; the last two are the same chosen target, once with nothing
//! listening behind it and once with the workload's own server listening — so what moves the
//! count is the CHOICE of a target and not the status the client was answered with.
//!
//! Every request is driven over the data plane by the address the load balancer PUBLISHED, every
//! resource is written through the control plane's own operations, and every datapoint is read
//! out of the CloudWatch store those series live in — the same store `GetMetricStatistics` reads
//! — so what a row asserts is what a client's own read would answer. The server behind the
//! registered target is published where the service that RUNS a workload publishes one
//! (`service_common::served_http_addresses`), which is the seam the load balancer resolves a
//! registration through.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::service_common::served_http_addresses::{AddressAnswer, ServedAddresses, ServesHttp};

use crate::alb_data_plane::{
    add_rule, create_listener, create_load_balancer, fixed_response, forward_to, health_reaching,
    ip_target_group, path_pattern, redirect_to, register_address, seed_workload_interface, send,
    settle, WORKLOAD_ADDRESS, WORKLOAD_VPC,
};
use crate::alb_request_metrics::{
    expected, load_balancer_dimension, series_sum, target_group_dimension, watched_sums,
    LOAD_BALANCER_DIMENSION, TARGET_GROUP_DIMENSION,
};
use crate::provisioning_loop::emulator;

/// The series every row asserts by itself, beside the whole catalog.
const REQUEST_COUNT: &str = "RequestCount";

/// The paths the rules below answer: one per terminating action, and one `forward` to each of
/// the two groups.
const FIXED_PATH: &str = "/answered-by-the-listener";
const REDIRECT_PATH: &str = "/moved";
const EMPTY_GROUP_PATH: &str = "/to-an-empty-group";
const WORKLOAD_PATH: &str = "/to-the-workload";

/// What the `fixed-response` rule answers, and what the workload's own server answers.
const FIXED_BODY: &str = "answered-by-the-load-balancer-itself";
const TARGET_BODY: &str = "served-by-the-registered-workload";

/// The port the registration receives traffic on, which is the port the workload's server is
/// published at.
const TARGET_PORT: i32 = 80;

/// The host the `redirect` action sends the client to, and the status it answers with.
const REDIRECT_HOST: &str = "elsewhere.test";
const REDIRECT_STATUS: &str = "HTTP_301";

/// The server the workload behind the registered address listens with, published exactly where
/// the service running such a workload publishes one.
#[derive(Debug)]
struct Workload;

impl ServesHttp for Workload {
    fn serve(&self, _method: &str, _path: &str) -> Option<AddressAnswer> {
        Some(AddressAnswer {
            status: 200,
            content_type: Some("text/plain".to_string()),
            body: TARGET_BODY.as_bytes().to_vec(),
        })
    }
}

/// One request of the table: what it asks for, what the load balancer must answer, whether the
/// workload's own server is listening while it is made, and what the series must read afterwards.
struct Row {
    case: &'static str,
    path: &'static str,
    status: u16,
    /// A fragment the body must carry, for a row whose answer has a body of its own.
    body: Option<&'static str>,
    /// Whether the workload behind the registered address is listening for this row.
    serving: bool,
    /// The load balancer's own catalog after this row, cumulative over the table.
    load_balancer: &'static [(&'static str, f64)],
    /// The catalog of the group holding the registration, cumulative over the table.
    registered_group: &'static [(&'static str, f64)],
}

/// RULE: a request is counted into `RequestCount` exactly when the load balancer chose a target
/// for it.
#[tokio::test]
async fn a_request_is_counted_as_processed_only_where_a_target_was_chosen() {
    let (router, engine) = emulator();
    seed_workload_interface(&engine);

    let (lb_arn, published) = create_load_balancer(&router, "alb-request-count").await;
    let listener_arn = create_listener(&router, &lb_arn).await;
    let empty_group = ip_target_group(&router, "request-count-empty", WORKLOAD_VPC).await;
    let registered_group = ip_target_group(&router, "request-count-live", WORKLOAD_VPC).await;
    register_address(
        &router,
        "the workload's own address",
        &registered_group,
        WORKLOAD_ADDRESS,
    )
    .await;

    let mut fields = path_pattern(FIXED_PATH);
    fields.extend(fixed_response("200", FIXED_BODY, None));
    add_rule(&router, "the fixed response", &listener_arn, 10, &fields).await;
    let mut fields = path_pattern(REDIRECT_PATH);
    fields.extend(redirect_to(REDIRECT_STATUS, REDIRECT_HOST));
    add_rule(&router, "the redirect", &listener_arn, 20, &fields).await;
    let mut fields = path_pattern(EMPTY_GROUP_PATH);
    fields.extend(forward_to(&empty_group));
    add_rule(&router, "the forward to an empty group", &listener_arn, 30, &fields).await;
    let mut fields = path_pattern(WORKLOAD_PATH);
    fields.extend(forward_to(&registered_group));
    add_rule(&router, "the forward to the workload", &listener_arn, 40, &fields).await;

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    // The registration is one the load balancer sends requests to, so the two forward rows
    // below are forwards to a CHOSEN target rather than to a group with nothing in it.
    health_reaching(
        &router,
        "the workload's own address",
        &registered_group,
        "healthy",
    )
    .await;

    let load_balancer_dimensions = [(LOAD_BALANCER_DIMENSION, load_balancer_dimension(&lb_arn))];
    let registered_dimensions = [
        (TARGET_GROUP_DIMENSION, target_group_dimension(&registered_group)),
        (LOAD_BALANCER_DIMENSION, load_balancer_dimension(&lb_arn)),
    ];
    let empty_dimensions = [
        (TARGET_GROUP_DIMENSION, target_group_dimension(&empty_group)),
        (LOAD_BALANCER_DIMENSION, load_balancer_dimension(&lb_arn)),
    ];

    assert_eq!(
        series_sum(&engine, REQUEST_COUNT, &load_balancer_dimensions),
        0.0,
        "before any traffic: a load balancer that has answered nothing counts no request"
    );

    let rows: &[Row] = &[
        Row {
            case: "a fixed-response action: the listener answered, so no target was chosen",
            path: FIXED_PATH,
            status: 200,
            body: Some(FIXED_BODY),
            serving: false,
            load_balancer: &[("HTTP_Fixed_Response_Count", 1.0)],
            registered_group: &[],
        },
        Row {
            case: "a redirect action: the same, with the redirection's own class",
            path: REDIRECT_PATH,
            status: 301,
            body: None,
            serving: false,
            load_balancer: &[
                ("HTTP_Fixed_Response_Count", 1.0),
                ("HTTP_Redirect_Count", 1.0),
                ("HTTPCode_ELB_3XX_Count", 1.0),
            ],
            registered_group: &[],
        },
        Row {
            case: "a forward whose group has no target to send the request to: nothing was \
                   chosen, so nothing was processed",
            path: EMPTY_GROUP_PATH,
            status: 503,
            body: None,
            serving: false,
            load_balancer: &[
                ("HTTP_Fixed_Response_Count", 1.0),
                ("HTTP_Redirect_Count", 1.0),
                ("HTTPCode_ELB_3XX_Count", 1.0),
                ("HTTPCode_ELB_5XX_Count", 1.0),
                ("HTTPCode_ELB_503_Count", 1.0),
            ],
            registered_group: &[],
        },
        Row {
            case: "a forward whose chosen target has nothing listening: the target was chosen, \
                   so the request is processed",
            path: WORKLOAD_PATH,
            status: 502,
            body: None,
            serving: false,
            load_balancer: &[
                ("RequestCount", 1.0),
                ("HTTP_Fixed_Response_Count", 1.0),
                ("HTTP_Redirect_Count", 1.0),
                ("HTTPCode_ELB_3XX_Count", 1.0),
                ("HTTPCode_ELB_5XX_Count", 2.0),
                ("HTTPCode_ELB_502_Count", 1.0),
                ("HTTPCode_ELB_503_Count", 1.0),
                ("TargetConnectionErrorCount", 1.0),
            ],
            registered_group: &[
                ("RequestCount", 1.0),
                ("TargetConnectionErrorCount", 1.0),
            ],
        },
        Row {
            case: "the same forward with the workload's server listening: the target answered, \
                   and the request is processed once more",
            path: WORKLOAD_PATH,
            status: 200,
            body: Some(TARGET_BODY),
            serving: true,
            load_balancer: &[
                ("RequestCount", 2.0),
                ("HTTP_Fixed_Response_Count", 1.0),
                ("HTTP_Redirect_Count", 1.0),
                ("HTTPCode_ELB_3XX_Count", 1.0),
                ("HTTPCode_ELB_5XX_Count", 2.0),
                ("HTTPCode_ELB_502_Count", 1.0),
                ("HTTPCode_ELB_503_Count", 1.0),
                ("HTTPCode_Target_2XX_Count", 1.0),
                ("TargetConnectionErrorCount", 1.0),
            ],
            registered_group: &[
                ("RequestCount", 2.0),
                ("HTTPCode_Target_2XX_Count", 1.0),
                ("TargetConnectionErrorCount", 1.0),
            ],
        },
    ];

    // What keeps the workload's server listening for the rows that need it: published once the
    // table reaches them, exactly as the service running the workload publishes one.
    let addresses = ServedAddresses::of(&*engine).expect("the shared engine vends the records");
    let mut listening = None;

    for row in rows {
        if row.serving && listening.is_none() {
            listening = Some(addresses.publish(WORKLOAD_ADDRESS, TARGET_PORT, Arc::new(Workload)));
        }
        let answered = send(&router, "GET", &published, row.path, &[]).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} body {:?}",
            row.case, row.status, answered.status, answered.body
        );
        if let Some(fragment) = row.body {
            assert!(
                answered.body.contains(fragment),
                "case {:?}: expected the body to carry {fragment:?}, got {:?}",
                row.case,
                answered.body
            );
        }
        let sums = watched_sums(&engine, &load_balancer_dimensions);
        assert_eq!(
            sums,
            expected(row.load_balancer),
            "case {:?}: expected the load balancer's catalog to read {:?}, got {sums:?}",
            row.case,
            expected(row.load_balancer)
        );
        let narrowed = watched_sums(&engine, &registered_dimensions);
        assert_eq!(
            narrowed,
            expected(row.registered_group),
            "case {:?}: expected the registered group's catalog to read {:?}, got {narrowed:?}",
            row.case,
            expected(row.registered_group)
        );
        // The group that answered NOTHING is counted into nothing at all, however many requests
        // its action's sibling rules answered.
        let idle = series_sum(&engine, REQUEST_COUNT, &empty_dimensions);
        assert_eq!(
            idle, 0.0,
            "case {:?}: expected the group that chose no target to count no request, got {idle}",
            row.case
        );
    }

    drop(listening);
}
