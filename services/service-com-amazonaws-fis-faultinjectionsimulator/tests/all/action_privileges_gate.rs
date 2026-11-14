// SPDX-License-Identifier: CC0-1.0

//! WHICH documents decide whether an experiment's action may be carried out, and WHAT in
//! them authorizes it.
//!
//! FIS runs the actions of an experiment under an IAM role, so an action whose own API call
//! that role is not allowed to make cannot be carried out: the run reaches the terminal
//! status `failed` and its `ExperimentState` attributes the failure to insufficient
//! privileges. Two halves of that gate are pinned here, both of them about the INPUTS the
//! decision is taken from rather than about the policy evaluation itself (which
//! `core/router/tests/all/fis_execution_role_permissions.rs` owns, against the real IAM
//! operations).
//!
//! RULE (the grants): an action's own API call may be authorized by more than one
//! documented IAM action — the AWS Service Authorization Reference publishes both an
//! `ec2:`-namespaced grant for the whole API-fault family and one `fis:`-namespaced grant
//! per fault that family injects — so a role allowed ANY one of the grants an action
//! documents may carry it out, and only a role allowed NONE of them may not. An action for
//! which one grant is documented is unchanged by that: the one grant is the only one that
//! authorizes it.
//!
//! RULE (the role): the documents that decide are the ones in force on the role FIS
//! PERFORMS the action under, which is what the experiment's account targeting says — the
//! template's own execution role for a single-account run, and the role each targeted
//! account's target account configuration names for a multi-account one, where FIS reaches
//! the targeted account through that configuration's role. So a multi-account run is
//! decided by the target-account roles, whatever the template's own role is or is not
//! allowed, and a refusal names the role whose documents withheld the grant, which is the
//! policy a client has to change.
//!
//! Both rules are reachable only through `StartExperiment` followed by a read of the
//! experiment, so every row drives them there, over the router. The roles, their trust
//! policies and the policies in force on them are seeded into the IAM store in the SAME
//! shared engine — the records `CreateRole` and `PutRolePolicy` write — because that store
//! is what the gate reads.
//!
//! The grant rows differ along what that rule generalises over: which action the template
//! carries (three of the API-fault family, plus an action outside it whose entry documents
//! a single grant), which of the documented grants the role is allowed (the `ec2:` one, the
//! `fis:` one for the fault the action injects, the `fis:` one for ANOTHER fault of the
//! family, all three of them at once, a wildcard covering them, and a grant for something
//! else entirely), and the resource the statement is scoped to (every resource, and the
//! experiment the reference gives those grants as their resource type). The role rows
//! differ along the account targeting, how many accounts are configured, which of the
//! configured accounts' roles carry the grant, whether the template's own role carries it,
//! and whether the account holds a record of the configured role at all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::state::{
    IamState, FIS_SERVICE_PRINCIPAL, INSUFFICIENT_PRIVILEGES_CODE,
};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn as iam_role_arn, InlinePolicy, DEFAULT_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, poll_until_final, send, start_experiment, token, FALLBACK_REGION,
};

/// The grant the FIS actions reference states for the whole API-fault family, and the one
/// a refusal of such an action names — it is the spelling documented for every action of
/// the family. Spelled out here rather than imported from the crate, so a changed catalog
/// fails this suite.
const EC2_INJECT_API_ERROR: &str = "ec2:InjectApiError";

/// The FIS-namespaced grant the reference publishes for the fault
/// `aws:fis:inject-api-internal-error` injects.
const FIS_INJECT_API_INTERNAL_ERROR: &str = "fis:InjectApiInternalError";

/// See [`FIS_INJECT_API_INTERNAL_ERROR`], for the throttle fault.
const FIS_INJECT_API_THROTTLE_ERROR: &str = "fis:InjectApiThrottleError";

/// See [`FIS_INJECT_API_INTERNAL_ERROR`], for the unavailable fault.
const FIS_INJECT_API_UNAVAILABLE_ERROR: &str = "fis:InjectApiUnavailableError";

/// The action ids of the API-fault family, each injecting its own fault.
const INTERNAL_ERROR_ACTION: &str = "aws:fis:inject-api-internal-error";
/// See [`INTERNAL_ERROR_ACTION`].
const THROTTLE_ERROR_ACTION: &str = "aws:fis:inject-api-throttle-error";
/// See [`INTERNAL_ERROR_ACTION`].
const UNAVAILABLE_ERROR_ACTION: &str = "aws:fis:inject-api-unavailable-error";

/// An action OUTSIDE that family, whose catalog entry documents a single grant: the rule
/// about several grants must leave it deciding on that one.
const ALARM_ACTION: &str = "aws:cloudwatch:assert-alarm-state";

/// The one grant the FIS actions reference states for [`ALARM_ACTION`].
const ALARM_PERMISSION: &str = "cloudwatch:DescribeAlarms";

/// The alarm [`ALARM_ACTION`] names: a value the gate compares against policy statements
/// and never dials.
const ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-privileges";

/// The resource type the reference gives the FIS-namespaced grants of the API-fault
/// family: the EXPERIMENT. A policy scoped this way states something about the call.
const EXPERIMENT_SCOPED_RESOURCE: &str = "arn:*:fis:*:*:experiment/*";

/// Every resource, which is how the recorded policies of these actions were written.
const EVERY_RESOURCE: &str = "*";

/// The IAM role the API-fault actions here TARGET, named by ARN so the target resolves to
/// it. It is also a resource the action is decided against.
const TARGETED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-privileges-target";

/// The name every template here gives its single action, which a privileges failure
/// locates (`actions.<name>`).
const ACTION_NAME: &str = "injectAction";

/// The target key the API-fault family takes its roles under, as the action catalog
/// publishes it.
const ROLES_TARGET: &str = "Roles";

/// How long a row waits for its run to reach a terminal status. The emulator's whole
/// compressed run window plus its transitions is a fraction of this.
const TERMINAL_BUDGET: Duration = Duration::from_secs(30);

/// How many experiments may be active at once (the published FIS quota), which is how many
/// rows are started before any of them is waited for.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// The first account a multi-account row configures, and the second — ascending, so the
/// order the configurations are held in is the order they are written here.
const FIRST_ACCOUNT: &str = "111111111111";
/// See [`FIRST_ACCOUNT`].
const SECOND_ACCOUNT: &str = "222222222222";

/// The one statement in force on a role: the IAM actions it allows, and the resource it
/// allows them on.
type Statement = (&'static [&'static str], &'static str);

/// An emulator carrying this service over one shared engine, plus that engine: the gate
/// reads the IAM store that lives in the SAME engine, so a case seeds its roles there.
fn emulator_with_iam_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A trust policy that lets FIS assume the role, so no row is refused for the trust half of
/// the execution-role gate.
fn fis_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": FIS_SERVICE_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy whose single statement allows `statement`'s actions on its resource.
fn allow(statement: Statement) -> String {
    let (actions, resource) = statement;
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// Seed the role `role_name` under the ARN `arn` into `store` — the record `CreateRole`
/// writes — with `document` as an embedded role policy, the record `PutRolePolicy` writes.
///
/// The ARN is passed in rather than derived, because a target account configuration names a
/// role of ANOTHER account and the gate resolves a role by the ARN its record carries.
fn seed_role(store: &mut IamState, role_name: &str, arn: &str, document: String) {
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.to_string(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: Some(fis_trust()),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    store.role_inline_policies.insert(
        name_key(role_name),
        BTreeMap::from([(
            name_key("privileges"),
            InlinePolicy {
                policy_name: "privileges".to_string(),
                policy_document: document,
            },
        )]),
    );
}

/// A create body for a template whose single action is `action_id`, run under `role_arn`
/// and carrying `options` as its `experimentOptions` when there are any.
///
/// An action of the API-fault family carries the parameters its catalog entry records as
/// required and names an `aws:iam:role` target by ARN; the alarm action carries its own two
/// required parameters and names no target.
fn template_body(label: &str, role_arn: &str, action_id: &str, options: Option<Value>) -> Value {
    let (targets, action) = if action_id == ALARM_ACTION {
        (
            json!({}),
            json!({
                "actionId": action_id,
                "parameters": { "alarmArns": ALARM_ARN, "alarmStates": "OK" }
            }),
        )
    } else {
        (
            json!({
                ROLES_TARGET: {
                    "resourceType": "aws:iam:role",
                    "resourceArns": [TARGETED_ROLE_ARN],
                    "selectionMode": "ALL"
                }
            }),
            json!({
                "actionId": action_id,
                "parameters": {
                    "duration": "PT1M",
                    "service": "ec2",
                    "operations": "DescribeRegions",
                    "percentage": "10"
                },
                "targets": { ROLES_TARGET: ROLES_TARGET }
            }),
        )
    };
    let mut body = json!({
        "clientToken": token(&format!("privileges-template-{label}")),
        "description": format!("the gate's deciding documents: {label}"),
        "roleArn": role_arn,
        "stopConditions": [{ "source": "none" }],
        "targets": targets,
        "actions": { ACTION_NAME: action },
    });
    if let Some(options) = options {
        body["experimentOptions"] = options;
    }
    body
}

/// Create the template `body` describes and answer its minted id.
async fn template_id(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let created = create_template(router, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
        .to_string()
}

/// Start an experiment from `template_id` and answer its minted id.
async fn experiment_id(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
    template_id: &str,
) -> String {
    let started = start_experiment(
        router,
        json!({
            "clientToken": token(&format!("privileges-start-{label}")),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected the start to be accepted, got {} with {:?}",
        started.status, started.body
    );
    started.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {:?}", started.body))
        .to_string()
}

/// Assert that `state` is the state of a run no role's documents showed anything against:
/// it reached the terminal status of an elapsed run and attributes no failure.
fn assert_ran(case: &str, action_id: &str, state: &Value) {
    assert_eq!(
        state["status"].as_str().unwrap_or_default(),
        "completed",
        "case {case:?}: expected the run of {action_id} to reach completed, got {state:?}"
    );
    assert!(
        state["error"].is_null(),
        "case {case:?}: expected a run that was not refused a privilege to report no \
         error, got {state:?}"
    );
}

/// Assert that `state` is the state of a run refused the grant `permission` on the role
/// `role_arn`, with the whole attribution AWS words such a failure with.
fn assert_refused(case: &str, action_id: &str, role_arn: &str, permission: &str, state: &Value) {
    assert_eq!(
        state["status"].as_str().unwrap_or_default(),
        "failed",
        "case {case:?}: expected the run of {action_id} to fail, got {state:?}"
    );
    assert_eq!(
        state["error"]["code"].as_str().unwrap_or_default(),
        INSUFFICIENT_PRIVILEGES_CODE,
        "case {case:?}: expected the failure attributed to privileges, got {state:?}"
    );
    assert_eq!(
        state["error"]["location"].as_str().unwrap_or_default(),
        format!("actions.{ACTION_NAME}"),
        "case {case:?}: expected the error to locate the action that could not run, \
         got {state:?}"
    );
    let reason = state["reason"].as_str().unwrap_or_default();
    let expected = format!(
        "Not enough privileges to run action {ACTION_NAME} ({action_id}): the experiment \
         role {role_arn} is not allowed to perform {permission}"
    );
    assert_eq!(
        reason, expected,
        "case {case:?}: expected the reason {expected:?}, got {reason:?}"
    );
}

/// One row of the grant rule: the action a template carries, the one statement in force on
/// its execution role, and the grant a refusal names — `None` for a run the documents
/// authorize.
struct GrantRow {
    case: &'static str,
    action_id: &'static str,
    allowed: Statement,
    refused_permission: Option<&'static str>,
}

/// The role one grant row's template runs under.
fn grant_role_name(index: usize) -> String {
    format!("fis-privileges-exec-{index}")
}

fn grant_rows() -> Vec<GrantRow> {
    vec![
        GrantRow {
            case: "the ec2-namespaced grant, on every resource",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&[EC2_INJECT_API_ERROR], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            case: "the fis-namespaced grant for the fault injected, on every resource",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&[FIS_INJECT_API_INTERNAL_ERROR], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            case: "the fis-namespaced grant scoped to the experiment resource type",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&[FIS_INJECT_API_INTERNAL_ERROR], EXPERIMENT_SCOPED_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            case: "all three fis-namespaced grants, scoped to the experiment resource type",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (
                &[
                    FIS_INJECT_API_INTERNAL_ERROR,
                    FIS_INJECT_API_THROTTLE_ERROR,
                    FIS_INJECT_API_UNAVAILABLE_ERROR,
                ],
                EXPERIMENT_SCOPED_RESOURCE,
            ),
            refused_permission: None,
        },
        GrantRow {
            case: "a wildcard covering the fis-namespaced grants",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&["fis:InjectApi*"], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            case: "the fis-namespaced grant of the throttle fault, for the throttle action",
            action_id: THROTTLE_ERROR_ACTION,
            allowed: (&[FIS_INJECT_API_THROTTLE_ERROR], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            case: "the fis-namespaced grant of the unavailable fault, for that action",
            action_id: UNAVAILABLE_ERROR_ACTION,
            allowed: (&[FIS_INJECT_API_UNAVAILABLE_ERROR], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            // A grant of the family, but for ANOTHER fault: no documented grant of THIS
            // action is allowed, so the role may not carry it out.
            case: "the fis-namespaced grant of another fault of the family",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&[FIS_INJECT_API_THROTTLE_ERROR], EVERY_RESOURCE),
            refused_permission: Some(EC2_INJECT_API_ERROR),
        },
        GrantRow {
            case: "two fis-namespaced grants, neither of them this action's",
            action_id: UNAVAILABLE_ERROR_ACTION,
            allowed: (
                &[FIS_INJECT_API_INTERNAL_ERROR, FIS_INJECT_API_THROTTLE_ERROR],
                EVERY_RESOURCE,
            ),
            refused_permission: Some(EC2_INJECT_API_ERROR),
        },
        GrantRow {
            case: "a grant for something else entirely",
            action_id: INTERNAL_ERROR_ACTION,
            allowed: (&["logs:PutLogEvents"], EVERY_RESOURCE),
            refused_permission: Some(EC2_INJECT_API_ERROR),
        },
        GrantRow {
            // An action whose entry documents ONE grant: that grant authorizes it.
            case: "the single documented grant of an action outside the family",
            action_id: ALARM_ACTION,
            allowed: (&[ALARM_PERMISSION], EVERY_RESOURCE),
            refused_permission: None,
        },
        GrantRow {
            // And no other grant does, however many grants another action documents.
            case: "an api-fault grant for an action outside the family",
            action_id: ALARM_ACTION,
            allowed: (&[FIS_INJECT_API_INTERNAL_ERROR], EVERY_RESOURCE),
            refused_permission: Some(ALARM_PERMISSION),
        },
    ]
}

/// RULE: a role allowed ANY one of the grants an action documents may carry that action
/// out, and only a role allowed none of them is refused it — which an action documenting a
/// single grant is unchanged by.
#[tokio::test]
async fn any_one_documented_grant_authorizes_an_action() {
    let (router, engine) = emulator_with_iam_store();
    let rows = grant_rows();

    {
        let state = engine.state::<IamState>();
        let mut store = state.lock();
        for (index, row) in rows.iter().enumerate() {
            let name = grant_role_name(index);
            let arn = iam_role_arn(DEFAULT_PATH, &name);
            seed_role(&mut store, &name, &arn, allow(row.allowed));
        }
    }

    // Started in groups no larger than the active-experiment quota, so no row is refused
    // for a quota another row's run is holding.
    let mut ended: Vec<(usize, Value)> = Vec::new();
    let indices: Vec<usize> = (0..rows.len()).collect();
    for group in indices.chunks(ACTIVE_EXPERIMENTS_QUOTA) {
        let mut started: Vec<(usize, String)> = Vec::new();
        for &index in group {
            let row = &rows[index];
            let role_arn = iam_role_arn(DEFAULT_PATH, &grant_role_name(index));
            let label = format!("grant-{index}");
            let body = template_body(&label, &role_arn, row.action_id, None);
            let template = template_id(&router, row.case, body).await;
            started.push((
                index,
                experiment_id(&router, row.case, &label, &template).await,
            ));
        }
        for (index, id) in started {
            let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
            ended.push((index, experiment["state"].clone()));
        }
    }

    for (index, state) in &ended {
        let row = &rows[*index];
        let role_arn = iam_role_arn(DEFAULT_PATH, &grant_role_name(*index));
        match row.refused_permission {
            None => assert_ran(row.case, row.action_id, state),
            Some(permission) => {
                assert_refused(row.case, row.action_id, &role_arn, permission, state);
            }
        }
    }
}

/// Which role's documents a row expects the verdict to come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decider {
    /// The template's own execution role.
    TemplateRole,
    /// The role the configuration for this account id names.
    AccountRole(&'static str),
}

/// One row of the role rule: the statement in force on the template's execution role, the
/// target account configurations the template holds, and the role a refusal must be
/// attributed to — `None` for a run no role's documents showed anything against.
struct ActingRow {
    case: &'static str,
    template_grant: Statement,
    /// In creation order: the account id, and the statement in force on the role that
    /// account's configuration names — `None` for a role the account holds no record of.
    configurations: &'static [(&'static str, Option<Statement>)],
    refused_by: Option<Decider>,
}

/// The template's own execution role for one acting row.
fn acting_template_role(index: usize) -> String {
    format!("fis-privileges-template-{index}")
}

/// The role one acting row's configuration for `account` names. It belongs to that
/// account, which is what a target account configuration requires of it.
fn acting_target_role(index: usize, account: &str) -> (String, String) {
    let name = format!("fis-privileges-account-{index}-{account}");
    let arn = format!("arn:aws:iam::{account}:role/{name}");
    (name, arn)
}

fn acting_rows() -> Vec<ActingRow> {
    /// The template role of a multi-account run carries exactly this in the recorded
    /// wiring: it may assume the role the targeted account's configuration names, and
    /// nothing else.
    const ASSUME_ONLY: Statement = (&["sts:AssumeRole"], EVERY_RESOURCE);
    /// A role allowed to carry out the action under test.
    const INJECTS: Statement = (&[FIS_INJECT_API_THROTTLE_ERROR], EVERY_RESOURCE);
    /// A readable document carrying no grant for the action under test.
    const UNRELATED: Statement = (&["logs:PutLogEvents"], EVERY_RESOURCE);

    vec![
        ActingRow {
            case: "a single-account run whose template role holds the grant",
            template_grant: INJECTS,
            configurations: &[],
            refused_by: None,
        },
        ActingRow {
            case: "a single-account run whose template role holds no grant",
            template_grant: UNRELATED,
            configurations: &[],
            refused_by: Some(Decider::TemplateRole),
        },
        ActingRow {
            case: "a multi-account run whose template role may only assume the target role",
            template_grant: ASSUME_ONLY,
            configurations: &[(FIRST_ACCOUNT, Some(INJECTS))],
            refused_by: None,
        },
        ActingRow {
            // The template role carries the grant and the target role does not: the run is
            // refused, and attributed to the role that withheld it.
            case: "a multi-account run whose target role holds no grant",
            template_grant: (&[EC2_INJECT_API_ERROR], EVERY_RESOURCE),
            configurations: &[(FIRST_ACCOUNT, Some(UNRELATED))],
            refused_by: Some(Decider::AccountRole(FIRST_ACCOUNT)),
        },
        ActingRow {
            case: "a multi-account run where every targeted account's role holds the grant",
            template_grant: ASSUME_ONLY,
            configurations: &[(FIRST_ACCOUNT, Some(INJECTS)), (SECOND_ACCOUNT, Some(INJECTS))],
            refused_by: None,
        },
        ActingRow {
            case: "a multi-account run where the second account's role holds no grant",
            template_grant: ASSUME_ONLY,
            configurations: &[
                (FIRST_ACCOUNT, Some(INJECTS)),
                (SECOND_ACCOUNT, Some(UNRELATED)),
            ],
            refused_by: Some(Decider::AccountRole(SECOND_ACCOUNT)),
        },
        ActingRow {
            // Nothing can be shown about a role the account holds no record of, so the run
            // goes ahead — the same reading the gate takes of any unreadable document.
            case: "a multi-account run whose target role the account holds no record of",
            template_grant: ASSUME_ONLY,
            configurations: &[(FIRST_ACCOUNT, None)],
            refused_by: None,
        },
    ]
}

/// Create the target account configuration for `account` on `template`, naming `role_arn`.
async fn create_configuration(
    router: &MultiServiceRouter,
    case: &str,
    label: &str,
    template: &str,
    account: &str,
    role_arn: &str,
) {
    let answer = send(
        router,
        "POST",
        &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
        Some(json!({
            "roleArn": role_arn,
            "clientToken": token(&format!("privileges-config-{label}-{account}"))
        })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the configuration for {account} to be created, got {} \
         with {:?}",
        answer.status, answer.body
    );
}

/// RULE: the documents that decide an action are the ones in force on the role FIS performs
/// it under — the template's execution role for a single-account run, and each targeted
/// account's configured role for a multi-account one — and a refusal names that role.
#[tokio::test]
async fn the_deciding_role_is_the_one_the_action_is_performed_under() {
    let (router, engine) = emulator_with_iam_store();
    let rows = acting_rows();

    {
        let state = engine.state::<IamState>();
        let mut store = state.lock();
        for (index, row) in rows.iter().enumerate() {
            let name = acting_template_role(index);
            let arn = iam_role_arn(DEFAULT_PATH, &name);
            seed_role(&mut store, &name, &arn, allow(row.template_grant));
            for (account, grant) in row.configurations {
                let Some(grant) = grant else {
                    continue;
                };
                let (name, arn) = acting_target_role(index, account);
                seed_role(&mut store, &name, &arn, allow(*grant));
            }
        }
    }

    let mut ended: Vec<(usize, Value)> = Vec::new();
    let indices: Vec<usize> = (0..rows.len()).collect();
    for group in indices.chunks(ACTIVE_EXPERIMENTS_QUOTA) {
        let mut started: Vec<(usize, String)> = Vec::new();
        for &index in group {
            let row = &rows[index];
            let label = format!("acting-{index}");
            let role_arn = iam_role_arn(DEFAULT_PATH, &acting_template_role(index));
            let options = (!row.configurations.is_empty())
                .then(|| json!({ "accountTargeting": "multi-account" }));
            let body = template_body(&label, &role_arn, THROTTLE_ERROR_ACTION, options);
            let template = template_id(&router, row.case, body).await;
            for (account, _) in row.configurations {
                let (_, arn) = acting_target_role(index, account);
                create_configuration(&router, row.case, &label, &template, account, &arn).await;
            }
            started.push((
                index,
                experiment_id(&router, row.case, &label, &template).await,
            ));
        }
        for (index, id) in started {
            let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
            ended.push((index, experiment["state"].clone()));
        }
    }

    for (index, state) in &ended {
        let row = &rows[*index];
        let template_role = iam_role_arn(DEFAULT_PATH, &acting_template_role(*index));
        let Some(decider) = row.refused_by else {
            assert_ran(row.case, THROTTLE_ERROR_ACTION, state);
            continue;
        };
        let deciding_role = match decider {
            Decider::TemplateRole => template_role.clone(),
            Decider::AccountRole(account) => acting_target_role(*index, account).1,
        };
        assert_refused(
            row.case,
            THROTTLE_ERROR_ACTION,
            &deciding_role,
            EC2_INJECT_API_ERROR,
            state,
        );
        // The verdict is attributed to the role that withheld the grant and to no other
        // role of the run, which is how a client learns which policy to change.
        let reason = state["reason"].as_str().unwrap_or_default();
        let mut others: Vec<String> = row
            .configurations
            .iter()
            .map(|(account, _)| acting_target_role(*index, account).1)
            .collect();
        others.push(template_role);
        for other in others.iter().filter(|other| **other != deciding_role) {
            assert!(
                !reason.contains(other.as_str()),
                "case {:?}: expected the reason to name only {deciding_role:?}, \
                 got {reason:?}",
                row.case
            );
        }
    }
}
