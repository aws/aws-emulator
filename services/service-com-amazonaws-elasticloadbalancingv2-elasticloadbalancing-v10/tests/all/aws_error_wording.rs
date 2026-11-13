// SPDX-License-Identifier: CC0-1.0

//! The WORDS and the CODE a refusal carries, for the refusals whose message is built from
//! the request rather than from the resource the request happens to name.
//!
//! An error reply's `<Code>` and `<Message>` are part of the contract: a client branches on
//! the code and shows the message, so a refusal that fires at the right moment with the
//! right status is still wrong when it states the emulator's own prose. The rule each table
//! below asserts is one rule of the service, so each table spans what that rule
//! generalizes over rather than the one call that showed it:
//!
//! 1. A priority another rule of the listener already evaluates at is refused under
//!    `PriorityInUse` by both operations that place a rule at a priority — the listener is
//!    not part of either refusal — each naming the ONE priority the request asked for, where
//!    a `SetRulePriorities` pair naming no rule at all is refused about the SET it addresses
//!    ([`a_priority_already_in_use_is_refused_naming_only_the_priority`]).
//! 2. A listener's default rule refuses the three operations that address a rule, each with
//!    the sentence for what it would have changed
//!    ([`the_default_rule_guards_state_what_cannot_be_changed`]).
//! 3. A port of a load balancer that already has a listener is refused naming the LOAD
//!    BALANCER, not the port
//!    ([`a_taken_listener_port_is_refused_naming_the_load_balancer`]).
//! 4. A request carrying more certificate ARNs than one call may name is refused on the
//!    COUNT — stating the limit the model fixes and the count the request sent — whatever
//!    listener protocol it names, for every operation that takes a `Certificates` list
//!    ([`more_certificates_than_one_call_may_carry_are_refused_on_the_count`]).
//! 5. A request the framework refuses before any handler runs — an absent required
//!    parameter as well as a value outside a modelled bound — is answered under this API's
//!    ONE input-refusal code
//!    ([`every_boundary_refusal_is_answered_under_one_code`]).
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, an `<ErrorResponse>` envelope out — on the harness `provisioning_loop.rs` and
//! `listener_rules.rs` already carry, so nothing here builds a request a second way. The
//! refusals that are relayed from the EC2 call a placement makes need both services on one
//! engine and are tabled in
//! `core/router/tests/all/elbv2_subnet_and_elastic_ip_resolution.rs`.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, create_rule, forward_to, Form,
};
use crate::provisioning_loop::{action, call, create_http_target_group, emulator, p, Reply};
use crate::rule_deletion_and_condition_value_spellings::default_rule_arn;
use crate::target_group_attributes_and_modification::unescaped;

/// The awsQuery code of `PriorityInUseException`
/// (`smithy_query(query="operation",operation="CreateRule")`).
const PRIORITY_IN_USE: &str = "PriorityInUse";

/// The awsQuery code of `RuleNotFoundException`, which `SetRulePriorities` models
/// (`smithy_query(query="operation",operation="SetRulePriorities")`).
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery code of `OperationNotPermittedException`, which `DeleteRule`, `ModifyRule`
/// and `SetRulePriorities` each model.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery code of `DuplicateListenerException`.
const DUPLICATE_LISTENER: &str = "DuplicateListener";

/// The awsQuery code of `TooManyCertificatesException`, which `CreateListener` and
/// `ModifyListener` model.
const TOO_MANY_CERTIFICATES: &str = "TooManyCertificates";

/// The code this API answers an input it cannot accept with — the one code for a rule it
/// documents in prose and for a value the framework refused at the boundary alike.
const VALIDATION_ERROR: &str = "ValidationError";

/// The sentence `SetRulePriorities` refuses a PAIR NAMING NO RULE with: there the refusal is
/// about the set of priorities the request asked for rather than about one value, which the
/// error code tells apart from the conflict below.
const SET_PRIORITIES_REFUSAL: &str = "One or more priorities not found";

/// How many certificate ARNs one request may carry, from the documentation of the
/// `Certificates` member every operation that takes one declares: "You must provide
/// exactly one certificate" (`CreateListener`, `ModifyListener`) and "You can specify one
/// certificate per call" (`AddListenerCertificates`).
const CERTIFICATES_PER_CALL: usize = 1;

/// A certificate ARN to hand a listener. The ARN FORM is what this service resolves, so
/// any Certificate Manager certificate ARN does; `index` makes each one distinct.
fn certificate_arn(index: usize) -> String {
    format!("arn:aws:acm:us-west-2:000000000000:certificate/{index:08}-1111-1111-1111-111111111111")
}

/// The status, the awsQuery `<Code>` and the `<Message>` of one refusal, with the
/// serializer's escaping undone so an assertion compares the VALUE and not its encoding.
fn refusal(reply: &Reply) -> (u16, String, String) {
    (
        reply.status,
        reply.error_code(),
        unescaped(&reply.text("Message").unwrap_or_default()),
    )
}

/// Assert one refusal: its status, its code and its whole message.
fn assert_refused(case: &str, reply: &Reply, code: &str, message: &str) {
    assert_eq!(
        refusal(reply),
        (400, code.to_string(), message.to_string()),
        "case {case:?}: expected 400 {code} {message:?}, got {}",
        reply.body
    );
}

/// The priorities the first table drives: the lowest `RulePriority` admits, the one the
/// recorded call used, one in between and the highest it admits
/// (`smithy_query(query="operation",operation="CreateRule")`: `Priority` range 1..50000).
const PRIORITIES: [i32; 4] = [1, 10, 999, 50_000];

/// The fields of one acceptable rule: a path condition of its own and a forward action.
///
/// `CreateRule` models `Conditions` and `Actions` both required, so every row that must be
/// ACCEPTED carries them; the path keeps each seeded rule distinct.
fn rule_fields(target_group_arn: &str, path: &str) -> Form {
    let mut fields = vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p("Conditions.member.1.Values.member.1", path),
    ];
    fields.extend(forward_to(target_group_arn));
    fields
}

/// Rule 1: a priority a rule of the listener already evaluates at is refused under the
/// code the operation models for it, with the sentence that operation words it in.
///
/// "A listener can't have multiple rules with the same priority" is one rule of the
/// service, so both operations that place a rule at a priority — `CreateRule` and
/// `SetRulePriorities` — refuse it under `PriorityInUse` with the same sentence: the
/// priority the request asked for, which is the value the caller has to change. Neither
/// names the listener whose namespace it is.
#[tokio::test]
async fn a_priority_already_in_use_is_refused_naming_only_the_priority() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "priority-wording-alb").await;
    let tg = create_http_target_group(&router, "priority-wording-tg").await;
    let listener = create_listener(&router, &lb, &tg, 80).await;

    // One rule per priority under test, plus the one `SetRulePriorities` moves.
    for priority in PRIORITIES {
        add_rule(
            &router,
            "seeding the priorities under test",
            &listener,
            priority,
            &rule_fields(&tg, &format!("/seeded/{priority}")),
        )
        .await;
    }
    let moved = add_rule(
        &router,
        "the rule SetRulePriorities moves",
        &listener,
        7,
        &rule_fields(&tg, "/moved"),
    )
    .await;

    for priority in PRIORITIES {
        let expected = format!("Priority '{priority}' is currently in use");

        let created = create_rule(
            &router,
            &listener,
            &priority.to_string(),
            &rule_fields(&tg, &format!("/second/{priority}")),
        )
        .await;
        assert_refused(
            &format!("CreateRule at the taken priority {priority}"),
            &created,
            PRIORITY_IN_USE,
            &expected,
        );

        let mut form = action("SetRulePriorities");
        form.push(p("RulePriorities.member.1.RuleArn", &moved));
        form.push(p("RulePriorities.member.1.Priority", &priority.to_string()));
        let set = call(&router, &form).await;
        assert_refused(
            &format!("SetRulePriorities onto the taken priority {priority}"),
            &set,
            PRIORITY_IN_USE,
            &expected,
        );

        // The listener the priority lives in is NOT part of the refusal: the message is
        // the one above, which quotes nothing but the priority.
        for reply in [&created, &set] {
            let message = unescaped(&reply.text("Message").unwrap_or_default());
            assert!(
                !message.contains(&listener),
                "case the taken priority {priority}: the refusal must not name the \
                 listener, got {message:?}"
            );
        }
    }

    // The complement, which is what the sentence above is told apart from: a pair naming a
    // rule this store does not hold is a refusal about the SET of priorities the request
    // addresses rather than about one priority, under its own code.
    let mut form = action("SetRulePriorities");
    form.push(p("RulePriorities.member.1.RuleArn", "not-a-rule-arn"));
    form.push(p("RulePriorities.member.1.Priority", "5"));
    let missing = call(&router, &form).await;
    assert_refused(
        "SetRulePriorities naming a rule the store does not hold",
        &missing,
        RULE_NOT_FOUND,
        SET_PRIORITIES_REFUSAL,
    );
}

/// One row of the default-rule table: the operation, the fields it addresses the default
/// rule with, and the sentence the refusal states for it.
struct DefaultRuleGuard {
    operation: &'static str,
    fields: Form,
    message: String,
}

/// What each of the three operations that address a rule answers for a listener's default
/// rule.
fn default_rule_guards(rule_arn: &str) -> Vec<DefaultRuleGuard> {
    vec![
        DefaultRuleGuard {
            operation: "DeleteRule",
            fields: vec![p("RuleArn", rule_arn)],
            message: format!("Default rule '{rule_arn}' cannot be deleted"),
        },
        DefaultRuleGuard {
            operation: "ModifyRule",
            fields: vec![
                p("RuleArn", rule_arn),
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.Values.member.1", "/anything"),
            ],
            message: format!("Default rule '{rule_arn}' cannot be modified"),
        },
        DefaultRuleGuard {
            operation: "SetRulePriorities",
            fields: vec![
                p("RulePriorities.member.1.RuleArn", rule_arn),
                p("RulePriorities.member.1.Priority", "5"),
            ],
            // The default rule has no numeric priority to move — it is reported as
            // `default` — so this refusal names no rule at all.
            message: "The priority of a default rule cannot be changed".to_string(),
        },
    ]
}

/// Rule 2: a listener's default rule refuses the three operations that address a rule,
/// each with the sentence for what that operation would have changed.
///
/// The table crosses the three operations with the default rules of several listeners, so
/// the ARN each message quotes is the one the REQUEST named rather than one listener's.
#[tokio::test]
async fn the_default_rule_guards_state_what_cannot_be_changed() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "default-rule-wording-alb").await;
    let tg = create_http_target_group(&router, "default-rule-wording-tg").await;

    for port in [80, 8080, 9000] {
        let listener = create_listener(&router, &lb, &tg, port).await;
        let default_rule = default_rule_arn(&router, &listener).await;
        for guard in default_rule_guards(&default_rule) {
            let mut form = action(guard.operation);
            form.extend(guard.fields.iter().cloned());
            let reply = call(&router, &form).await;
            assert_refused(
                &format!("{} on the default rule of the listener on {port}", guard.operation),
                &reply,
                OPERATION_NOT_PERMITTED,
                &guard.message,
            );
        }
        // The guard refused and changed nothing: the default rule is still there.
        assert_eq!(
            default_rule_arn(&router, &listener).await,
            default_rule,
            "case the listener on {port}: its default rule must survive every refusal"
        );
    }
}

/// Rule 3: a port of a load balancer that already has a listener is refused naming the
/// load balancer.
///
/// The table crosses several ports with two load balancers: the message is the same
/// sentence for every port of one load balancer — the port the request carried is not part
/// of it — and names the load balancer the request addressed.
#[tokio::test]
async fn a_taken_listener_port_is_refused_naming_the_load_balancer() {
    let (router, _engine) = emulator();
    let tg = create_http_target_group(&router, "duplicate-listener-tg").await;
    // A second target group, so the repeat carries DIFFERENT settings and is the duplicate
    // rather than the idempotent repeat the model documents.
    let other_tg = create_http_target_group(&router, "duplicate-listener-tg-two").await;

    for name in ["duplicate-listener-alb-one", "duplicate-listener-alb-two"] {
        let lb = create_load_balancer(&router, name).await;
        let expected =
            format!("A listener already exists on this port for this load balancer '{lb}'");
        for port in [80, 443, 8080] {
            create_listener(&router, &lb, &tg, port).await;
            let mut form = action("CreateListener");
            form.push(p("LoadBalancerArn", &lb));
            form.push(p("Protocol", "HTTP"));
            form.push(p("Port", &port.to_string()));
            form.push(p("DefaultActions.member.1.Type", "forward"));
            form.push(p("DefaultActions.member.1.TargetGroupArn", &other_tg));
            let reply = call(&router, &form).await;
            assert_refused(
                &format!("a second listener on port {port} of {name}"),
                &reply,
                DUPLICATE_LISTENER,
                &expected,
            );
        }
    }
}

/// The sentence a request carrying more certificate ARNs than one call may name is refused
/// with: the limit the model fixes, and the count the request carried.
fn certificate_count_message(requested: usize) -> String {
    format!(
        "Up to '{CERTIFICATES_PER_CALL}' certificate ARNs can be specified, but \
         '{requested}' were specified"
    )
}

/// Rule 4: a request carrying more certificate ARNs than one call may name is refused on
/// the count, under the code the operation's own model declares for it.
///
/// The table crosses several counts past the bound with every operation that takes a
/// `Certificates` list, and with the listener protocols the brackets differ for: the count
/// is a rule of the REQUEST, so it is what the refusal states whether the listener
/// terminates TLS or not. Exactly one certificate is the control row — it must be
/// accepted — and the published bound itself is the boundary.
#[tokio::test]
async fn more_certificates_than_one_call_may_carry_are_refused_on_the_count() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "certificate-count-alb").await;
    let tg = create_http_target_group(&router, "certificate-count-tg").await;

    // `CreateListener`: one certificate on a TLS-terminating listener is accepted, and any
    // count past the bound is the modelled count refusal — including on a listener whose
    // protocol carries no certificate at all, because the count is judged first.
    let mut port = 20_000;
    for (case, protocol, count) in [
        ("an HTTPS listener with one certificate", "HTTPS", 1),
        ("an HTTPS listener with two certificates", "HTTPS", 2),
        ("a TLS listener with three certificates", "TLS", 3),
        ("an HTTPS listener with five certificates", "HTTPS", 5),
        ("an HTTP listener with two certificates", "HTTP", 2),
    ] {
        port += 1;
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", protocol));
        form.push(p("Port", &port.to_string()));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
        for index in 0..count {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                &certificate_arn(index),
            ));
        }
        let reply = call(&router, &form).await;
        if count <= CERTIFICATES_PER_CALL {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the listener to be created, got {} {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_refused(
            case,
            &reply,
            TOO_MANY_CERTIFICATES,
            &certificate_count_message(count),
        );
    }

    // `ModifyListener` judges the same count on the listener the call leaves behind.
    let listener = create_listener(&router, &lb, &tg, 80).await;
    for (case, count) in [
        ("ModifyListener to HTTPS with two certificates", 2),
        ("ModifyListener to HTTPS with four certificates", 4),
    ] {
        let mut form = action("ModifyListener");
        form.push(p("ListenerArn", &listener));
        form.push(p("Protocol", "HTTPS"));
        for index in 0..count {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                &certificate_arn(index),
            ));
        }
        let reply = call(&router, &form).await;
        assert_refused(
            case,
            &reply,
            TOO_MANY_CERTIFICATES,
            &certificate_count_message(count),
        );
    }

    // `AddListenerCertificates` takes one certificate per call. Its model declares no
    // error for the per-call count, so the refusal is this API's input-refusal code with
    // the same sentence — and the count is judged before the listener's protocol, so the
    // HTTP listener below is refused for the count and not for its protocol.
    let https = {
        let mut form = action("CreateListener");
        form.push(p("LoadBalancerArn", &lb));
        form.push(p("Protocol", "HTTPS"));
        form.push(p("Port", "443"));
        form.push(p("Certificates.member.1.CertificateArn", &certificate_arn(0)));
        form.push(p("DefaultActions.member.1.Type", "forward"));
        form.push(p("DefaultActions.member.1.TargetGroupArn", &tg));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case creating the HTTPS listener the certificate list hangs on: expected 200, \
             got {} {}",
            reply.status, reply.body
        );
        reply
            .text("ListenerArn")
            .expect("the created HTTPS listener reports its ARN")
    };
    for (case, target, count, accepted) in [
        ("one certificate to an HTTPS listener", &https, 1, true),
        ("two certificates to an HTTPS listener", &https, 2, false),
        ("three certificates to an HTTPS listener", &https, 3, false),
        ("two certificates to an HTTP listener", &listener, 2, false),
    ] {
        let mut form = action("AddListenerCertificates");
        form.push(p("ListenerArn", target));
        for index in 0..count {
            form.push(p(
                &format!("Certificates.member.{}.CertificateArn", index + 1),
                &certificate_arn(index + 10),
            ));
        }
        let reply = call(&router, &form).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case adding {case}: expected 200, got {} {}",
                reply.status, reply.body
            );
            continue;
        }
        assert_refused(
            &format!("adding {case}"),
            &reply,
            VALIDATION_ERROR,
            &certificate_count_message(count),
        );
    }
}

/// One row of the boundary table: the case, the operation and the fields that make the
/// framework refuse the request before any handler runs.
struct BoundaryCase {
    case: &'static str,
    operation: &'static str,
    fields: Form,
}

/// The requests the framework refuses before a handler runs: an absent required parameter
/// — found while READING the request — and a value outside a bound the model declares,
/// found while walking the request that was read. Both classes, over several operations
/// and several members.
fn boundary_cases(listener_arn: &str, load_balancer_arn: &str) -> Vec<BoundaryCase> {
    vec![
        BoundaryCase {
            case: "CreateRule with no Priority",
            operation: "CreateRule",
            fields: vec![
                p("ListenerArn", listener_arn),
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.Values.member.1", "/a"),
                p("Actions.member.1.Type", "forward"),
            ],
        },
        BoundaryCase {
            case: "CreateRule with no ListenerArn",
            operation: "CreateRule",
            fields: vec![
                p("Priority", "1"),
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.Values.member.1", "/a"),
                p("Actions.member.1.Type", "forward"),
            ],
        },
        BoundaryCase {
            case: "CreateListener with no LoadBalancerArn",
            operation: "CreateListener",
            fields: vec![p("Protocol", "HTTP"), p("Port", "80")],
        },
        BoundaryCase {
            case: "DeleteRule with no RuleArn",
            operation: "DeleteRule",
            fields: Vec::new(),
        },
        BoundaryCase {
            // `Priority` is modelled `range 1..50000`, so nought is one past the floor.
            case: "CreateRule at a priority below the modelled range",
            operation: "CreateRule",
            fields: vec![
                p("ListenerArn", listener_arn),
                p("Priority", "0"),
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.Values.member.1", "/a"),
                p("Actions.member.1.Type", "forward"),
            ],
        },
        BoundaryCase {
            // `Port` is modelled `range 1..65535`, so one past its ceiling.
            case: "CreateListener on a port above the modelled range",
            operation: "CreateListener",
            fields: vec![
                p("LoadBalancerArn", load_balancer_arn),
                p("Protocol", "HTTP"),
                p("Port", "65536"),
            ],
        },
    ]
}

/// Rule 5: every refusal the framework raises before a handler runs is answered under this
/// API's one input-refusal code.
///
/// An absent required parameter and a value outside a modelled bound are the same class of
/// failure to a caller — the request has to be corrected — and the code is what a client
/// branches on, so both answer `400 ValidationError` with the validation envelope. The
/// table drives both classes over several operations and members.
#[tokio::test]
async fn every_boundary_refusal_is_answered_under_one_code() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "boundary-wording-alb").await;
    let tg = create_http_target_group(&router, "boundary-wording-tg").await;
    let listener = create_listener(&router, &lb, &tg, 80).await;

    for boundary in boundary_cases(&listener, &lb) {
        let mut form = action(boundary.operation);
        form.extend(boundary.fields.iter().cloned());
        let reply = call(&router, &form).await;
        let (status, code, message) = refusal(&reply);
        assert_eq!(
            (status, code.as_str()),
            (400, VALIDATION_ERROR),
            "case {:?}: expected 400 {VALIDATION_ERROR}, got {}",
            boundary.case,
            reply.body
        );
        // The sentence is the validation envelope AWS words these in, naming what was
        // refused rather than the layer that refused it.
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {:?}: expected the validation envelope, got {message:?}",
            boundary.case
        );
    }
}
