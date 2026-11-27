// SPDX-License-Identifier: CC0-1.0

//! What a REGISTRATION settles as, and what the calls whose precondition is a working
//! application answer for one that did not settle successfully.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns, with the owner of the instances mounted beside this service where a row needs it
//! — `tests/all/ec2_instance_shutdown.rs` owns that one), so the restJson1 routing, the
//! boundary constraint validation, the handler and the response serialization all run
//! exactly as they do in the emulator binary. Nothing reaches into `crate::state`: a
//! registration's outcome is only believed when `GetApplication` and `GetOperation` report
//! it back.
//!
//! RULE under test: registering an SAP application is asynchronous and what it settles as
//! is the outcome of the DISCOVERY it runs on the Amazon EC2 instances the request named —
//! a Systems Manager document run on each of them. A discovery the owner of those
//! instances lets it carry out leaves the application `ACTIVATED` with a `SUCCESS`
//! discovery and its operation record `SUCCESS`; one it cannot leaves the application
//! `FAILED` with a `REGISTRATION_FAILED` discovery and the record `ERROR` carrying the
//! refusal the inner call answered with. Every call whose precondition is a discovered
//! application then answers that outcome, deterministically and from the application's own
//! state rather than from what else is in flight:
//!
//! * `StartApplication` refuses a FAILED registration with `ValidationException` (400),
//!   naming the application, and so does `StopApplication`: a call that MOVES the
//!   application's own `Status` has nothing to move for an application the service never
//!   discovered, and the `FAILED` status that registration settled on stands;
//! * `StartApplicationRefresh` and `StartConfigurationChecks` refuse an application that
//!   has not been registered successfully — one still registering as much as one that
//!   failed — with `ConflictException` (409), in fixed text that names nothing of the
//!   request, for EVERY caller whatever order they arrive in;
//! * `StopApplication` is SERVED while a registration is still running, because stopping
//!   an SAP application is no change to what that registration is establishing.
//!
//! Two refusals of the REGISTRATION itself belong to the same slice: a registration that
//! describes an application of another `ApplicationType` than the one it carries is refused
//! with `ValidationException` (400) stating what its type has to specify, and so is one the
//! account has no room for under its Applications quota — which tracks the applications
//! the Region holds at that instant, so deregistering one frees a slot.
//!
//! The rows span what the rules generalise over: whether the owner of the instance holds
//! it and in what state, both lengths the modelled instance-id form admits, which
//! precondition call is made, the switches a stop carries, how many callers race, the
//! application type, the forms the modelled `ApplicationId` admits, and whether a member a
//! type excludes is carried, carried empty or omitted.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::APPLICATIONS_QUOTA;

use crate::application_lifecycle::{
    is_operation_id, rendered, settle, valid_registration, Emulator, Reply, REGION,
};
use crate::database_status_discovery::{database_status, wait_until};
use crate::ec2_instance_shutdown::{registration, settle_wait, Emulator as WithEc2};
use crate::resource_tags::{application_arn, list_tags};
use crate::tag_resource::{pairs, tag};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code a call that cannot act on the application in the state it is
/// in is refused with (`smithy_query(query="operation",
/// operation="StartApplicationRefresh")`: `ConflictException` `@httpError 409`; every
/// control call of this service models the same error).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// The modelled error code an `ApplicationId` naming no registered application is refused
/// with where the operation models one (`smithy_query(query="operation",
/// operation="StartApplication")`: `ResourceNotFoundException` `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// An instance id EC2 holds no record of: it issues `i-` and 17 hexadecimal digits of a
/// counter that starts at one, so nothing in this emulator is ever launched under an
/// all-`f` counter value, while the id satisfies the modelled `InstanceId` form.
const UNHELD_LONG_INSTANCE: &str = "i-0ffffffffffffffff";

/// The same, in the SHORT form the modelled `InstanceId` also admits
/// (`^i-[\w\d]{8}$|^i-[\w\d]{17}$`).
const UNHELD_SHORT_INSTANCE: &str = "i-ffffffff";

/// An id inside the modelled `InstanceId` form that EC2 does not read as an instance id at
/// all, so its owner resolves it to no instance either.
const UNREADABLE_INSTANCE: &str = "i-0zzzzzzzzzzzzzzzz";

/// How this service states that the application a call names has not been registered
/// successfully: entirely fixed text, naming nothing of the request.
///
/// Spelled out here rather than taken from the crate, so the wording is pinned
/// independently of the one place the handlers build it in.
const NOT_REGISTERED_SUCCESSFULLY: &str =
    "The specified application has not been registered successfully";

/// How this service states that the registration of `application_id` failed: the id the
/// request carried is the only variable part.
fn failed_registration(application_id: &str) -> String {
    format!("The registration of application {application_id} is failed.")
}

/// What a registration of type `SAP_ABAP` has to specify, as this service states it: an
/// ABAP application runs on the SAP HANA database of another application, so it names that
/// database rather than carrying credentials of its own.
const ABAP_REQUIREMENT: &str = "The SID and database ARN of ASCS or Application Server must \
                                be specified for SAP applications with type SAP_ABAP.";

/// How this service states that the account has already registered every application it is
/// allotted in this Region.
fn quota_refusal(count: usize) -> String {
    format!(
        "Resource Limit Reached. Account {ACCOUNT} is only allotted {APPLICATIONS_QUOTA} \
         applications. Currently has {count} applications"
    )
}

/// The refusal a failed registration explains itself with: the error the inner Systems
/// Manager call that carries the discovery document answered, relayed as this service
/// relays it, with the per-run request id masked ([`without_request_id`]).
///
/// Spelled out here rather than taken from the crate, for the same reason
/// [`NOT_REGISTERED_SUCCESSFULLY`] is. `instance_id` and the Region come from the request
/// and the signing context; the identity is the service-linked role this service discovers
/// an application as.
///
/// `pub(crate)` because one discovery is what this crate runs, so the suite about what a
/// RE-discovery settles as (`tests/all/refresh_discovery.rs`) pins the same relayed
/// sentence rather than spelling a second copy of it.
pub(crate) fn discovery_refusal(instance_id: &str) -> String {
    format!(
        "Error executing SSM document - AccessDeniedException User: \
         arn:aws:sts::{ACCOUNT}:assumed-role/AWSServiceRoleForAWSSSMForSAP/ssm-sap is not \
         authorized to perform: ssm:SendCommand on resource: \
         arn:aws:ec2:{REGION}:{ACCOUNT}:instance/{instance_id} because no identity-based \
         policy allows the ssm:SendCommand action (Service: Ssm, Status Code: 400, Request \
         ID: <request-id>) (SDK Attempt Count: 1)"
    )
}

/// `relayed` with the request id of the inner call replaced by `<request-id>`, having
/// asserted it is the id form such a call reports.
///
/// The request id of one run is per-run information a client must not depend on, so what
/// the rows pin is everything around it.
///
/// `pub(crate)` for the same reason [`discovery_refusal`] is.
pub(crate) fn without_request_id(case: &str, relayed: &str) -> String {
    let marker = "Request ID: ";
    let Some((head, rest)) = relayed.split_once(marker) else {
        panic!("case {case:?}: expected the relayed refusal to report a request id, got {relayed:?}");
    };
    let Some((request_id, tail)) = rest.split_once(')') else {
        panic!("case {case:?}: expected the request id to be parenthesised, got {relayed:?}");
    };
    assert!(
        is_operation_id(request_id),
        "case {case:?}: expected a UUID request id, got {request_id:?}"
    );
    format!("{head}{marker}<request-id>){tail}")
}

/// One registration, and what the discovery it runs on the instance it names can do.
#[derive(Debug)]
struct Outcome {
    case: &'static str,
    /// `None` is the instance the row launches through EC2 itself, so the owner holds it
    /// and reports it running; `Some` is an id that owner resolves to no instance.
    instance: Option<&'static str>,
    /// The `ApplicationStatus` the registration settles as.
    status: &'static str,
    /// The `ApplicationDiscoveryStatus` it settles as.
    discovery_status: &'static str,
    /// The `OperationStatus` the record the registration filed settles as.
    operation_status: &'static str,
}

/// The rows: the states the owner of an instance can answer in, and the control.
const OUTCOMES: &[Outcome] = &[
    Outcome {
        case: "an instance the owner holds and reports running",
        instance: None,
        status: "ACTIVATED",
        discovery_status: "SUCCESS",
        operation_status: "SUCCESS",
    },
    Outcome {
        case: "a long-form id the owner holds no instance for",
        instance: Some(UNHELD_LONG_INSTANCE),
        status: "FAILED",
        discovery_status: "REGISTRATION_FAILED",
        operation_status: "ERROR",
    },
    Outcome {
        case: "a short-form id the owner holds no instance for",
        instance: Some(UNHELD_SHORT_INSTANCE),
        status: "FAILED",
        discovery_status: "REGISTRATION_FAILED",
        operation_status: "ERROR",
    },
    Outcome {
        case: "an id the owner does not read as an instance id at all",
        instance: Some(UNREADABLE_INSTANCE),
        status: "FAILED",
        discovery_status: "REGISTRATION_FAILED",
        operation_status: "ERROR",
    },
];

/// The `OperationId` a reply reports.
fn operation_id(case: &str, reply: &Value) -> String {
    reply
        .get("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {reply}"))
        .to_string()
}

/// Register `application_id` on `instance` and answer the `OperationId` the call filed.
async fn register(emulator: &WithEc2, case: &str, application_id: &str, instance: &str) -> String {
    let reply = emulator
        .ssm_sap(
            "/register-application",
            registration(application_id, instance, &[]),
        )
        .await;
    assert_eq!(
        reply.get("Application").and_then(|app| app.get("Status")),
        Some(&json!("REGISTERING")),
        "case {case:?}: an asynchronous registration answers REGISTERING, got {reply}"
    );
    operation_id(case, &reply)
}

/// The `(Status, DiscoveryStatus)` `GetApplication` reports for `application_id`.
async fn reported_outcome(emulator: &WithEc2, case: &str, application_id: &str) -> (String, String) {
    let reply = emulator
        .ssm_sap("/get-application", json!({"ApplicationId": application_id}))
        .await;
    let member = |name: &str| {
        reply
            .get("Application")
            .and_then(|app| app.get(name))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Application {name}, got {reply}"))
            .to_string()
    };
    (member("Status"), member("DiscoveryStatus"))
}

/// The `(Status, StatusMessage)` `GetOperation` reports for `operation_id`.
async fn reported_operation(
    emulator: &WithEc2,
    case: &str,
    operation_id: &str,
) -> (String, Option<String>) {
    let reply = emulator
        .ssm_sap("/get-operation", json!({"OperationId": operation_id}))
        .await;
    let operation = reply
        .get("Operation")
        .unwrap_or_else(|| panic!("case {case:?}: expected an Operation, got {reply}"));
    let status = operation
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Operation Status, got {reply}"))
        .to_string();
    let status_message = operation
        .get("StatusMessage")
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, status_message)
}

/// Register `application_id` on an instance the owner holds no record of and wait out the
/// clock, so its registration has FAILED — the state the preconditions below are about.
async fn register_failed(emulator: &WithEc2, case: &str, application_id: &str) {
    register(emulator, case, application_id, UNHELD_LONG_INSTANCE).await;
    tokio::time::sleep(settle_wait()).await;
    let (status, discovery_status) = reported_outcome(emulator, case, application_id).await;
    assert_eq!(
        (status.as_str(), discovery_status.as_str()),
        ("FAILED", "REGISTRATION_FAILED"),
        "case {case:?}: setup expected a failed registration, got {status}/{discovery_status}"
    );
}

/// RULE: a registration settles as the discovery it runs on the instance it named could —
/// and the record it filed carries the refusal that discovery ran into.
#[tokio::test]
async fn a_registration_settles_as_its_discovery_on_the_named_instance_could() {
    let emulator = WithEc2::new();
    let launched = emulator.launch_running(1).await[0].clone();

    for (index, row) in OUTCOMES.iter().enumerate() {
        let case = row.case;
        let application_id = format!("outcome-{index}");
        let instance = row.instance.unwrap_or(launched.as_str());
        let operation = register(&emulator, case, &application_id, instance).await;

        // Before the clock brings the outcome due, every row reports the same thing: the
        // registration is running and its record explains nothing.
        let (status, discovery_status) = reported_outcome(&emulator, case, &application_id).await;
        let (operation_status, _) = reported_operation(&emulator, case, &operation).await;
        if (status.as_str(), operation_status.as_str()) == ("REGISTERING", "INPROGRESS") {
            assert_eq!(
                discovery_status, "REGISTERING",
                "case {case:?}: a running registration is still discovering, got \
                 {discovery_status}"
            );
        }

        tokio::time::sleep(settle_wait()).await;
        let (status, discovery_status) = reported_outcome(&emulator, case, &application_id).await;
        assert_eq!(
            (status.as_str(), discovery_status.as_str()),
            (row.status, row.discovery_status),
            "case {case:?}: expected {}/{}, got {status}/{discovery_status}",
            row.status,
            row.discovery_status
        );

        let (operation_status, status_message) =
            reported_operation(&emulator, case, &operation).await;
        assert_eq!(
            operation_status, row.operation_status,
            "case {case:?}: expected the record to settle {}, got {operation_status}",
            row.operation_status
        );
        match status_message {
            // A record that did its work explains nothing.
            None => assert_eq!(
                row.operation_status, "SUCCESS",
                "case {case:?}: a failed registration must explain itself"
            ),
            Some(relayed) => assert_eq!(
                without_request_id(case, &relayed),
                discovery_refusal(instance),
                "case {case:?}: expected the relayed refusal, got {relayed:?}"
            ),
        }

        // A registration that failed discovered nothing, so it has no components; one
        // that succeeded has the SAP HANA component its own System ID describes.
        let listed = emulator
            .ssm_sap("/list-components", json!({"ApplicationId": &application_id}))
            .await;
        let components = listed
            .get("Components")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            components.is_empty(),
            row.discovery_status == "REGISTRATION_FAILED",
            "case {case:?}: expected components only where discovery completed, got {listed}"
        );
    }
}

/// RULE: `StartApplication` refuses an application whose registration FAILED with
/// `ValidationException` (400) naming it, answers the modelled 404 for an id nothing
/// registered, and starts one whose registration succeeded.
#[tokio::test]
async fn a_start_answers_the_registration_outcome_of_the_application_it_names() {
    let emulator = WithEc2::new();
    let launched = emulator.launch_running(1).await[0].clone();

    // The rows span the registration outcome and the forms the modelled `ApplicationId`
    // admits, with an id nothing ever registered beside them.
    let failed = ["start-failed", "START.FAILED-1", "s"];
    for application_id in failed {
        register_failed(&emulator, "a failed registration", application_id).await;
        let reply = emulator
            .reply("/start-application", json!({"ApplicationId": application_id}))
            .await;
        assert_eq!(
            (reply.status, reply.code()),
            (400, VALIDATION_EXCEPTION),
            "case {application_id:?}: expected 400 {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert_eq!(
            message(&reply),
            failed_registration(application_id),
            "case {application_id:?}: expected the failed-registration refusal, got {:?}",
            message(&reply)
        );
        assert!(
            reply.member("OperationId").is_none(),
            "case {application_id:?}: a refused start hands out no OperationId, got {reply:?}"
        );
    }

    let never = "start-never-registered";
    let reply = emulator
        .reply("/start-application", json!({"ApplicationId": never}))
        .await;
    assert_eq!(
        (reply.status, reply.code()),
        (404, RESOURCE_NOT_FOUND_EXCEPTION),
        "case {never:?}: expected 404 {RESOURCE_NOT_FOUND_EXCEPTION}, got {reply:?}"
    );

    let discovered = "start-discovered";
    register(&emulator, discovered, discovered, &launched).await;
    tokio::time::sleep(settle_wait()).await;
    let reply = emulator
        .reply("/start-application", json!({"ApplicationId": discovered}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {discovered:?}: a discovered application starts, got {reply:?}"
    );
    assert!(
        reply.member("OperationId").is_some(),
        "case {discovered:?}: a served start files work, got {reply:?}"
    );
}

/// The `OperationId`s of the records of type `op_type` filed against `application_id`.
async fn records_of_type(
    emulator: &WithEc2,
    application_id: &str,
    op_type: &str,
) -> BTreeSet<String> {
    let reply = emulator
        .ssm_sap(
            "/list-operations",
            json!({
                "ApplicationId": application_id,
                "Filters": [{"Name": "Type", "Value": op_type, "Operator": "Equals"}],
            }),
        )
        .await;
    reply
        .get("Operations")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: every caller that asks a FAILED registration to refresh is refused the same way —
/// `ConflictException` (409) in the fixed wording — whatever order the callers arrive in,
/// and no refresh record is filed for any of them.
#[tokio::test]
async fn every_concurrent_refresh_of_a_failed_registration_is_refused_the_same_way() {
    let emulator = WithEc2::new();

    // The rows span how many callers race: one on its own, two, and the five of the
    // recorded contention.
    for racers in [1usize, 2, 5] {
        let case = format!("{racers} concurrent refreshes");
        let application_id = format!("refresh-contention-{racers}");
        register_failed(&emulator, &case, &application_id).await;

        let request = json!({"ApplicationId": &application_id});
        let mut answers: Vec<Reply> = Vec::with_capacity(racers);
        let mut pending = Vec::with_capacity(racers);
        for _ in 0..racers {
            pending.push(emulator.reply("/start-application-refresh", request.clone()));
        }
        for answer in pending {
            answers.push(answer.await);
        }

        for reply in &answers {
            assert_eq!(
                (reply.status, reply.code()),
                (409, CONFLICT_EXCEPTION),
                "case {case}: every racer is refused the same way, got {reply:?}"
            );
            assert_eq!(
                message(reply),
                NOT_REGISTERED_SUCCESSFULLY,
                "case {case}: expected the not-registered-successfully refusal, got {:?}",
                message(reply)
            );
            assert!(
                reply.member("OperationId").is_none(),
                "case {case}: a refused refresh hands out no OperationId, got {reply:?}"
            );
        }
        let filed = records_of_type(&emulator, &application_id, "START_APPLICATION_REFRESH").await;
        assert!(
            filed.is_empty(),
            "case {case}: a refused refresh files no record, got {filed:?}"
        );
    }
}

/// RULE: `StartConfigurationChecks` refuses an application that has not been registered
/// successfully — one still registering as much as one that failed — with
/// `ConflictException` (409) in the fixed wording, and runs the checks of one whose
/// registration succeeded.
#[tokio::test]
async fn configuration_checks_wait_for_a_successful_registration() {
    let emulator = WithEc2::new();
    let launched = emulator.launch_running(1).await[0].clone();

    // A registration that failed, and one still running: neither has been registered
    // successfully, so both are refused in the same words. The failed one is set up first,
    // because waiting out its clock would settle the other.
    let failed = "checks-failed";
    register_failed(&emulator, failed, failed).await;
    let still_running = "checks-registering";
    register(&emulator, still_running, still_running, &launched).await;

    for application_id in [still_running, failed] {
        let reply = emulator
            .reply(
                "/start-configuration-checks",
                json!({"ApplicationId": application_id}),
            )
            .await;
        assert_eq!(
            (reply.status, reply.code()),
            (409, CONFLICT_EXCEPTION),
            "case {application_id:?}: expected 409 {CONFLICT_EXCEPTION}, got {reply:?}"
        );
        assert_eq!(
            message(&reply),
            NOT_REGISTERED_SUCCESSFULLY,
            "case {application_id:?}: expected the not-registered-successfully refusal, got {:?}",
            message(&reply)
        );
        assert!(
            reply.member("ConfigurationCheckOperations").is_none(),
            "case {application_id:?}: a refusal files no run, got {reply:?}"
        );
    }

    // The same call on an application whose registration succeeded is served, so the
    // refusal is about the precondition and not about the operation.
    let discovered = "checks-discovered";
    register(&emulator, discovered, discovered, &launched).await;
    tokio::time::sleep(settle_wait()).await;
    let reply = emulator
        .reply(
            "/start-configuration-checks",
            json!({"ApplicationId": discovered}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {discovered:?}: a discovered application runs its checks, got {reply:?}"
    );
}

/// RULE: a stop is SERVED against an application whose registration is still running — it
/// answers the `OperationId` that tracks it, the application stays usable, and the
/// discovery result the registration owed is still reported.
#[tokio::test]
async fn a_stop_is_served_while_the_registration_is_still_running() {
    // The rows span the application type and the switches the stop carries: both are
    // independent of the registration still being under way.
    let rows: &[(&str, &str, Value)] = &[
        ("stop-registering-hana", "HANA", json!({})),
        (
            "stop-registering-abap",
            "SAP_ABAP",
            json!({"StopConnectedEntity": "DBMS"}),
        ),
        (
            "stop-registering-shutdown",
            "HANA",
            json!({"IncludeEc2InstanceShutdown": false}),
        ),
    ];

    for (application_id, app_type, switches) in rows {
        let emulator = Emulator::new();
        let registered = emulator
            .send(
                "/register-application",
                valid_registration(application_id, app_type),
            )
            .await;
        assert_eq!(
            registered.status, 200,
            "case {application_id:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        assert_eq!(
            registered.app_str("Status"),
            "REGISTERING",
            "case {application_id:?}: the registration must still be running, got {registered:?}"
        );

        let mut body = switches.as_object().cloned().expect("a JSON object");
        body.insert("ApplicationId".into(), json!(application_id));
        let stopped = emulator
            .send("/stop-application", Value::Object(body))
            .await;
        assert_eq!(
            stopped.status, 200,
            "case {application_id:?}: a stop is served while the registration runs, got \
             {stopped:?}"
        );
        let operation = stopped
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_operation_id(&operation),
            "case {application_id:?}: the stop answers the id that tracks it, got {operation:?}"
        );

        // The application resource stays fully usable: a tag attached to its ARN is
        // reported back under it.
        let arn = application_arn(REGION, application_id);
        let tagged = tag(&emulator, &arn, &pairs(&[("env", "dev")])).await;
        assert_eq!(
            tagged.status, 200,
            "case {application_id:?}: expected 200 from TagResource, got {tagged:?}"
        );
        let held = list_tags(&emulator, &arn).await;
        assert_eq!(
            held.status, 200,
            "case {application_id:?}: expected 200 from ListTagsForResource, got {held:?}"
        );

        // The stop takes over the delivery of what the registration owed rather than
        // discarding it: the discovery result is reported once the clock brings it due.
        settle().await;
        let read = emulator
            .send("/get-application", json!({"ApplicationId": application_id}))
            .await;
        assert_eq!(
            (read.app_str("Status"), read.app_str("DiscoveryStatus")),
            ("STOPPED", "SUCCESS"),
            "case {application_id:?}: expected the stop and the discovery result, got {read:?}"
        );
        let record = emulator
            .send("/get-operation", json!({"OperationId": operation}))
            .await;
        assert_eq!(
            record.op_str("Status"),
            "SUCCESS",
            "case {application_id:?}: the stop's own record completes, got {record:?}"
        );
    }
}

/// One registration, and whether it describes an application of the `ApplicationType` it
/// carries.
#[derive(Debug)]
struct TypeRule {
    case: &'static str,
    application_type: &'static str,
    /// What the row's `Credentials` member carries: `None` omits it, `Some(&[])` carries
    /// it empty — which specifies no credential, exactly as omitting it does.
    credentials: Option<&'static [&'static str]>,
    /// Whether the registration is accepted.
    accepted: bool,
}

/// The rows: the member one type excludes, carried, carried empty and omitted, on both
/// modelled types.
const TYPE_RULES: &[TypeRule] = &[
    TypeRule {
        case: "an ABAP registration carrying a credential",
        application_type: "SAP_ABAP",
        credentials: Some(&["HDB/HDB"]),
        accepted: false,
    },
    TypeRule {
        case: "an ABAP registration carrying two credentials",
        application_type: "SAP_ABAP",
        credentials: Some(&["SYSTEMDB", "TNT1"]),
        accepted: false,
    },
    TypeRule {
        case: "an ABAP registration carrying the member empty",
        application_type: "SAP_ABAP",
        credentials: Some(&[]),
        accepted: true,
    },
    TypeRule {
        case: "an ABAP registration omitting the member",
        application_type: "SAP_ABAP",
        credentials: None,
        accepted: true,
    },
    TypeRule {
        case: "a HANA registration carrying a credential",
        application_type: "HANA",
        credentials: Some(&["HDB"]),
        accepted: true,
    },
];

/// RULE: a registration carrying a member the `ApplicationType` it names excludes is
/// refused with `ValidationException` (400) stating what that type has to specify, and
/// leaves NOTHING behind.
#[tokio::test]
async fn a_registration_must_describe_an_application_of_the_type_it_carries() {
    for (index, row) in TYPE_RULES.iter().enumerate() {
        let emulator = Emulator::new();
        let case = row.case;
        let application_id = format!("type-rule-{index}");
        let mut body = valid_registration(&application_id, row.application_type);
        let members = body.as_object_mut().expect("a JSON object");
        if let Some(databases) = row.credentials {
            members.insert(
                "Credentials".into(),
                Value::Array(
                    databases
                        .iter()
                        .map(|database| {
                            json!({
                                "DatabaseName": database,
                                "CredentialType": "ADMIN",
                                "SecretId": format!("sap/{database}/admin"),
                            })
                        })
                        .collect(),
                ),
            );
        } else {
            members.remove("Credentials");
        }

        let reply = emulator.send("/register-application", body).await;
        if row.accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200 from RegisterApplication, got {reply:?}"
            );
            continue;
        }

        assert_eq!(
            (reply.status, reply.code()),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: expected 400 {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert_eq!(
            message(&reply),
            ABAP_REQUIREMENT,
            "case {case:?}: expected the type's requirement, got {:?}",
            message(&reply)
        );

        // The refused call left nothing behind: the application does not resolve, and the
        // refusal of that lookup is the one an id nothing registered gets.
        let fetched = emulator
            .send("/get-application", json!({"ApplicationId": &application_id}))
            .await;
        assert_eq!(
            (fetched.status, fetched.code()),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: a refused registration leaves no application, got {fetched:?}"
        );
        assert_eq!(
            message(&fetched),
            format!(
                "No registered application found for account: {ACCOUNT}, and application: \
                 {application_id}"
            ),
            "case {case:?}: expected the lookup refusal, got {:?}",
            message(&fetched)
        );
    }
}

/// RULE: an account may hold only the applications it is allotted per Region, the
/// registration that would exceed the allotment is refused with `ValidationException`
/// (400) and creates nothing, and the allotment tracks the LIVE count — so deregistering
/// one lets the identical call through, and another Region is unaffected.
#[tokio::test]
async fn the_applications_quota_refuses_the_registration_that_would_exceed_it() {
    let emulator = Emulator::new();

    // The allotment, filled exactly.
    let held: Vec<String> = (0..APPLICATIONS_QUOTA)
        .map(|index| format!("quota-{index:02}"))
        .collect();
    for application_id in &held {
        let reply = emulator
            .send(
                "/register-application",
                valid_registration(application_id, "HANA"),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {application_id:?}: a registration inside the allotment is served, got {reply:?}"
        );
    }

    // The one past it, and a second attempt under another id: the refusal is about the
    // account's count, not about the request.
    for application_id in ["quota-over", "quota-over-again"] {
        let reply = emulator
            .send(
                "/register-application",
                valid_registration(application_id, "HANA"),
            )
            .await;
        assert_eq!(
            (reply.status, reply.code()),
            (400, VALIDATION_EXCEPTION),
            "case {application_id:?}: expected 400 {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert_eq!(
            message(&reply),
            quota_refusal(APPLICATIONS_QUOTA),
            "case {application_id:?}: expected the quota refusal, got {:?}",
            message(&reply)
        );
        let fetched = emulator
            .send("/get-application", json!({"ApplicationId": application_id}))
            .await;
        assert!(
            fetched.status >= 400 && fetched.member("Application").is_none(),
            "case {application_id:?}: a refused registration creates nothing, got {fetched:?}"
        );
    }

    // The applications that reached the allotment stay registered.
    let listing = emulator.send("/list-applications", json!({})).await;
    let listed: BTreeSet<String> = listing
        .member("Applications")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("Id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        held.iter().cloned().collect::<BTreeSet<String>>(),
        "the applications that reached the allotment stay registered, got {listing:?}"
    );

    // Another Region is allotted its own: the count is per account AND Region.
    let elsewhere = emulator
        .send_in_region(
            "eu-west-1",
            "/register-application",
            valid_registration("quota-elsewhere", "HANA"),
        )
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "another Region has its own allotment, got {elsewhere:?}"
    );

    // Deregistering one frees a slot, so the identical call is served.
    settle().await;
    let removed = emulator
        .send("/deregister-application", json!({"ApplicationId": &held[0]}))
        .await;
    assert_eq!(
        removed.status, 200,
        "expected 200 from DeregisterApplication, got {removed:?}"
    );
    settle().await;
    let accepted = emulator
        .send(
            "/register-application",
            valid_registration("quota-over", "HANA"),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a freed slot admits the identical call, got {accepted:?}"
    );
}

/// One stop of an application whose registration FAILED, and the switches it carries.
#[derive(Debug)]
struct FailedStop {
    /// The `ApplicationId` of the application whose registration failed — the rows span
    /// the forms the modelled `ApplicationId` admits.
    application_id: &'static str,
    /// The `ApplicationId` of the control: the same stop against an application whose
    /// registration SUCCEEDED, so the refusal is about the registration outcome and not
    /// about the switches.
    control_id: &'static str,
    /// What the stop carries beyond the application it names.
    switches: &'static str,
    /// The `DatabaseStatus` the CONTROL's database reports once its served stop has
    /// settled: the DBMS it lives in, which a stop naming the connected entity took down
    /// with the application and a stop naming none left running.
    control_database: &'static str,
}

/// The rows: the switches `StopApplication` carries, each against both registration
/// outcomes — none, the connected entity, the Amazon EC2 shutdown, and both together.
const FAILED_STOPS: &[FailedStop] = &[
    FailedStop {
        application_id: "stop-failed",
        control_id: "stop-discovered",
        switches: "{}",
        control_database: "RUNNING",
    },
    FailedStop {
        application_id: "STOP.FAILED-1",
        control_id: "stop-discovered-dbms",
        switches: r#"{"StopConnectedEntity": "DBMS"}"#,
        control_database: "STOPPED",
    },
    FailedStop {
        application_id: "s",
        control_id: "stop-discovered-shutdown",
        switches: r#"{"IncludeEc2InstanceShutdown": true}"#,
        control_database: "RUNNING",
    },
    FailedStop {
        application_id: "stop-failed-both",
        control_id: "stop-discovered-both",
        switches: r#"{"StopConnectedEntity": "DBMS", "IncludeEc2InstanceShutdown": true}"#,
        control_database: "STOPPED",
    },
];

/// The `DatabaseId` the credential the shared registration builder declares derives, which
/// is the database every row below reads back
/// ([`crate::application_lifecycle::conditional_value`] names it, and a `DatabaseId` is the
/// canonical form of that name).
const STOP_DATABASE_ID: &str = "HDB";

/// The `/stop-application` body of `row` against `application_id`.
fn stop_body(row: &FailedStop, application_id: &str) -> Value {
    let mut body: Map<String, Value> = serde_json::from_str(row.switches)
        .unwrap_or_else(|error| panic!("case {:?}: the switches parse: {error}", row.switches));
    body.insert("ApplicationId".into(), json!(application_id));
    Value::Object(body)
}

/// RULE: a call that MOVES an application's own `Status` is refused with
/// `ValidationException` (400) naming the application when that application's registration
/// has settled as FAILED — the service discovered no SAP system to stop — so the refused
/// call files no work, moves no status and leaves nothing inside the application reporting
/// a running state the service never established.
///
/// Nothing here waits a fixed time: each outcome the clock owes is waited for as a
/// CONDITION ([`wait_until`]), and the control's own stop reaching `STOPPED` is what shows
/// a span has passed without the refused stop owing anything.
#[tokio::test]
async fn a_stop_is_refused_for_an_application_whose_registration_failed() {
    let emulator = WithEc2::new();
    // One held instance per control, so a row that asks for the Amazon EC2 shutdown
    // reaches an instance of its own.
    let launched = emulator
        .launch_running(i32::try_from(FAILED_STOPS.len()).expect("a small row count"))
        .await;

    // Every registration is filed before any of them is waited for, so the rows' clocks
    // run together rather than one after another.
    for (index, row) in FAILED_STOPS.iter().enumerate() {
        register(
            &emulator,
            row.application_id,
            row.application_id,
            UNHELD_LONG_INSTANCE,
        )
        .await;
        register(&emulator, row.control_id, row.control_id, &launched[index]).await;
    }

    for row in FAILED_STOPS {
        let case = row.application_id;
        wait_until(&emulator, case, row.application_id, "FAILED").await;
        wait_until(&emulator, row.control_id, row.control_id, "ACTIVATED").await;

        let settled = reported_outcome(&emulator, case, row.application_id).await;
        assert_eq!(
            (settled.0.as_str(), settled.1.as_str()),
            ("FAILED", "REGISTRATION_FAILED"),
            "case {case:?}: setup expected a failed registration, got {settled:?}"
        );

        // What the application and its database report before the stop is asked for.
        let before = emulator
            .reply(
                "/get-application",
                json!({"ApplicationId": row.application_id}),
            )
            .await;
        let database_before =
            database_status(&emulator, case, row.application_id, STOP_DATABASE_ID).await;
        assert_eq!(
            database_before, "UNKNOWN",
            "case {case:?}: nothing is known about the databases of an undiscovered \
             application, got {database_before}"
        );

        let refused = emulator
            .reply("/stop-application", stop_body(row, row.application_id))
            .await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: expected 400 {VALIDATION_EXCEPTION}, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            failed_registration(row.application_id),
            "case {case:?}: expected the failed-registration refusal, got {:?}",
            message(&refused)
        );
        assert!(
            refused.member("OperationId").is_none(),
            "case {case:?}: a refused stop hands out no OperationId, got {refused:?}"
        );

        // The refusal changed nothing: the application reports exactly what it did, and
        // no stop record was filed against it.
        let after = emulator
            .reply(
                "/get-application",
                json!({"ApplicationId": row.application_id}),
            )
            .await;
        assert_eq!(
            rendered(&after),
            rendered(&before),
            "case {case:?}: a refused stop leaves the application as it was, got {after:?}"
        );
        let filed = records_of_type(&emulator, row.application_id, "STOP_APPLICATION").await;
        assert!(
            filed.is_empty(),
            "case {case:?}: a refused stop files no record, got {filed:?}"
        );

        // The SAME stop against an application whose registration succeeded is served, so
        // the refusal is about the registration outcome and not about the switches.
        let served = emulator
            .reply("/stop-application", stop_body(row, row.control_id))
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a discovered application stops, got {served:?}"
        );
        let operation = served
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_operation_id(&operation),
            "case {case:?}: a served stop answers the id that tracks it, got {operation:?}"
        );
    }

    for row in FAILED_STOPS {
        let case = row.application_id;
        // The control's served stop reaching the status it owed is what shows the span
        // the refused stop would have taken has passed.
        wait_until(&emulator, row.control_id, row.control_id, "STOPPED").await;
        let control = reported_outcome(&emulator, row.control_id, row.control_id).await;
        assert_eq!(
            (control.0.as_str(), control.1.as_str()),
            ("STOPPED", "SUCCESS"),
            "case {:?}: a served stop completes, got {control:?}",
            row.control_id
        );
        let control_database =
            database_status(&emulator, row.control_id, row.control_id, STOP_DATABASE_ID).await;
        assert_eq!(
            control_database, row.control_database,
            "case {:?}: expected the DBMS of a discovered application to report {}, got \
             {control_database}",
            row.control_id, row.control_database
        );

        // The refused stop still owes nothing: the FAILED registration keeps the status it
        // settled on, and its database keeps reporting that nothing is known about it.
        let settled = reported_outcome(&emulator, case, row.application_id).await;
        assert_eq!(
            (settled.0.as_str(), settled.1.as_str()),
            ("FAILED", "REGISTRATION_FAILED"),
            "case {case:?}: a refused stop owes no status change, got {settled:?}"
        );
        let database_after =
            database_status(&emulator, case, row.application_id, STOP_DATABASE_ID).await;
        assert_eq!(
            database_after, "UNKNOWN",
            "case {case:?}: an undiscovered application's database reports no running \
             state, got {database_after}"
        );
    }
}
