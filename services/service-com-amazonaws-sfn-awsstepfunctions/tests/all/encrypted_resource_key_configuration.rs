// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the customer managed KMS key a Step Functions resource is
//! CONFIGURED on, driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions, KMS and IAM mounted on ONE engine, so the
//! key every decision here is taken against is the key KMS's own handlers hold.
//!
//! The rule under test: a resource whose `encryptionConfiguration.type` is
//! `CUSTOMER_MANAGED_KMS_KEY` uses the key the account named for every call that seals
//! or unseals that resource's data, so each such call is decided against the key KMS
//! holds — its key state first, then whether the authorities in force let the identity
//! the service makes the call as perform the cryptographic operation on it under the
//! encryption context naming that resource — and what the configuration keeps, and
//! every read reports, is the key's resolved ARN. A projection that reports none of the
//! sealed data (`includedData = METADATA_ONLY`) uses the key for nothing and is served
//! whatever the key's state and policy say.
//!
//! The rows span the operations that CONFIGURE a key (a state machine, an activity, an
//! update), the key states a key can be unusable in, the identities a call is decided
//! for (the account's own published credential, a bounded IAM user and a state machine's
//! execution role), the reference forms `kmsKeyId` accepts, the resources an encryption
//! context scopes a grant to, and both projections of a read.
//!
//! The harness (`Chain`, the key policies, `customer_managed`) is the one
//! `execution_data_encryption` builds; `tests/all.rs` is this binary's crate root, so
//! it is reached through `crate::<suite>`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;

use crate::activity_tasks::activity_definition;
use crate::encrypted_execution_writes::message;
use crate::execution_data_encryption::{
    customer_managed, granting_policy, withdrawn_policy, Chain, KMS_ACCESS_DENIED,
    KMS_ACCESS_DENIED_STATUS, SFN,
};
use crate::role_log_and_key_authorization::{data_key_policy, role};
use crate::state_machine_lifecycle::{
    error_code, string, ACCOUNT, HELLO_DEFINITION, PAST_THE_EXECUTION_DELAY, ROLE_ARN,
};

/// The terminal status a run that finished its own work reaches.
const SUCCEEDED: &str = "SUCCEEDED";

/// The terminal status a run the key stopped answering for reaches.
const FAILED: &str = "FAILED";

/// How long the definition below waits before it finishes, and a span past both that
/// wait and the delay an outcome settles after: the window in which an execution is
/// RUNNING with its outcome owed to the clock, which is when the key under it is moved.
const WAIT_SECONDS: u64 = 1;
const PAST_THE_WAIT: Duration =
    PAST_THE_EXECUTION_DELAY.saturating_add(Duration::from_secs(WAIT_SECONDS));

/// A definition that waits and then succeeds, so an execution of it is owed its outcome
/// for long enough to move the key under it.
const WAITING_DEFINITION: &str = r#"{"StartAt":"Pause","States":{"Pause":{"Type":"Wait","Seconds":1,"Next":"Done"},"Done":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// A definition whose one `Task` state starts an execution of `child_arn` — the nested
/// workflow the fire-and-forget integration begins and does not wait for.
fn nested_start_definition(child_arn: &str) -> String {
    json!({
        "StartAt": "CallChild",
        "States": {
            "CallChild": {
                "Type": "Task",
                "Resource": "arn:aws:states:::states:startExecution",
                "Parameters": { "StateMachineArn": child_arn },
                "End": true,
            }
        }
    })
    .to_string()
}

/// The modelled error class a configuration the service cannot apply answers, and the
/// status it carries: "received when encryptionConfiguration is specified but various
/// conditions exist which make the configuration invalid" (HTTP 400).
const INVALID_ENCRYPTION_CONFIGURATION: &str = "InvalidEncryptionConfiguration";
const REFUSAL_STATUS: u16 = 400;

/// The whole sentence that refusal carries for a key no cryptographic operation can
/// use: fixed service text naming the member at fault, with no value of the request in
/// it.
const KEY_NOT_ENABLED: &str = "kmsKeyId is not enabled";

/// The modelled error class a call the key's own state refuses answers.
const KMS_INVALID_STATE: &str = "KmsInvalidStateException";

/// The condition-key family a key policy scopes a grant to one resource's data with,
/// and the two entry names Step Functions binds its cryptographic operations to
/// (AWS KMS condition keys: "kms:EncryptionContext:<context-key>").
const STATE_MACHINE_CONTEXT_CONDITION: &str = "kms:EncryptionContext:aws:states:stateMachineArn";
const ACTIVITY_CONTEXT_CONDITION: &str = "kms:EncryptionContext:aws:states:activityArn";

/// The worker name a poll here identifies itself by, which the service only logs.
const WORKER: &str = "configuration-suite-worker";

/// The payload an execution is started on, read back through the projections below.
const PAYLOAD: &str = r#"{"marker":"configured-key","items":[1,"two"]}"#;

/// One key state that admits no cryptographic use of the key: how a caller gets a key
/// into it, and the call that takes it back out.
struct Unusable {
    case: &'static str,
    /// The `CreateKey` request the key is made with, beside its policy.
    create: Value,
    /// The KMS calls that put the created key into this key state.
    into: Vec<(&'static str, Value)>,
    /// The KMS calls that put it back into a usable key state — empty for a key state
    /// no call reverses, which is one the configuration can only ever be refused in.
    back: Vec<(&'static str, Value)>,
}

/// Every key state a configuration naming the key is refused in, and how a caller
/// reaches it. The states themselves are KMS's own `KeyState` values; the calls that
/// move a key between them are that service's own operations.
fn unusable_states(key: &str) -> Vec<Unusable> {
    vec![
        Unusable {
            case: "the key is disabled",
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT" }),
            into: vec![("DisableKey", json!({ "KeyId": key }))],
            back: vec![("EnableKey", json!({ "KeyId": key }))],
        },
        Unusable {
            case: "the key's deletion is scheduled",
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT" }),
            into: vec![(
                "ScheduleKeyDeletion",
                json!({ "KeyId": key, "PendingWindowInDays": 7 }),
            )],
            // Cancelling the deletion leaves the key disabled, so it takes both calls.
            back: vec![
                ("CancelKeyDeletion", json!({ "KeyId": key })),
                ("EnableKey", json!({ "KeyId": key })),
            ],
        },
        Unusable {
            case: "the key waits for key material of its own",
            // "Origin EXTERNAL" mints a key with no key material at all, which waits
            // in `PendingImport` from the moment it exists.
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT", "Origin": "EXTERNAL" }),
            into: Vec::new(),
            back: Vec::new(),
        },
    ]
}

/// Make `calls` on the key, each of which must be served.
async fn drive(chain: &Chain, calls: &[(&str, Value)]) {
    for (operation, body) in calls {
        chain.kms(operation, body).await;
    }
}

/// A key policy granting the account that owns the key `actions`, under `condition`
/// when one is given: the shape of the default key policy ("gives the Amazon Web
/// Services account that owns the KMS key permission to use it in all cryptographic
/// operations"), narrowed so one action or one encryption context can be granted while
/// everything else is withheld.
fn conditioned_policy(actions: &[&str], condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Sid": "EnableIAMUserPermissions",
        "Effect": "Allow",
        "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
        "Action": actions,
        "Resource": "*"
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// The `CreateStateMachine` request for `name` on `encryption`.
fn create_machine_request(name: &str, encryption: Option<Value>) -> Value {
    let mut request = json!({
        "name": name,
        "definition": HELLO_DEFINITION,
        "roleArn": ROLE_ARN,
    });
    if let Some(encryption) = encryption {
        request["encryptionConfiguration"] = encryption;
    }
    request
}

/// Whether a listing of this account's resources of `plural` carries `name`, read
/// through the service's OWN list operation: nothing may be created by a refused
/// configuration.
async fn is_listed(chain: &Chain, operation: &str, plural: &str, name: &str) -> bool {
    let listed = chain.sfn(operation, &json!({})).await;
    listed[plural]
        .as_array()
        .unwrap_or_else(|| panic!("case {operation}: expected a {plural} array, got {listed:?}"))
        .iter()
        .any(|item| item.get("name").and_then(Value::as_str) == Some(name))
}

/// A configuration naming a key in a key state that admits no cryptographic use of it
/// is refused with 400 `InvalidEncryptionConfiguration` carrying the service's fixed
/// sentence, and creates nothing — and the SAME request is served once the key is back
/// in a usable key state, so the refusal is the key's state at the instant of the call
/// and nothing latches.
#[tokio::test]
async fn a_configuration_naming_an_unusable_key_is_refused_and_creates_nothing() {
    // Each row: the operation, the name it would create, its request, and the listing
    // that proves nothing was created.
    for (index, state) in unusable_states("placeholder").into_iter().enumerate() {
        let chain = Chain::new();
        let mut create = state.create.clone();
        create["Policy"] = Value::String(granting_policy().to_string());
        let key = string(&chain.kms("CreateKey", &create).await["KeyMetadata"], "Arn");
        // The rows above are written against a placeholder, because the key they move
        // is the one this iteration just minted.
        let state = unusable_states(&key)
            .into_iter()
            .nth(index)
            .expect("case the rows: one row per key state");
        drive(&chain, &state.into).await;

        let machine_name = format!("unusable-machine-{index}");
        let activity_name = format!("unusable-activity-{index}");
        let rows: Vec<(&str, Value, &str, &str, &str)> = vec![
            (
                "CreateStateMachine",
                create_machine_request(&machine_name, Some(customer_managed(&key))),
                "ListStateMachines",
                "stateMachines",
                machine_name.as_str(),
            ),
            (
                "CreateActivity",
                json!({
                    "name": activity_name,
                    "encryptionConfiguration": customer_managed(&key)
                }),
                "ListActivities",
                "activities",
                activity_name.as_str(),
            ),
        ];

        for (operation, request, listing, plural, name) in &rows {
            let (status, answer) = chain.call(&SFN, operation, request).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (REFUSAL_STATUS, INVALID_ENCRYPTION_CONFIGURATION),
                "case {:?} / {operation}: a configuration naming an unusable key is refused, \
                 got {status} with {answer:?}",
                state.case
            );
            assert_eq!(
                message(&answer),
                KEY_NOT_ENABLED,
                "case {:?} / {operation}: expected the service's own sentence, got {answer:?}",
                state.case
            );
            assert!(
                !is_listed(&chain, listing, plural, name).await,
                "case {:?} / {operation}: a refused configuration creates nothing, but {name:?} \
                 is listed",
                state.case
            );
        }

        if state.back.is_empty() {
            continue;
        }
        drive(&chain, &state.back).await;
        for (operation, request, listing, plural, name) in &rows {
            let (status, answer) = chain.call(&SFN, operation, request).await;
            assert_eq!(
                status, 200,
                "case {:?} / {operation}: a key back in a usable key state serves the same \
                 configuration, got {status} with {answer:?}",
                state.case
            );
            assert!(
                is_listed(&chain, listing, plural, name).await,
                "case {:?} / {operation}: the served configuration created {name:?}",
                state.case
            );
        }
    }
}

/// The same rule on an UPDATE: moving a state machine onto a key in such a key state is
/// refused, and the state machine keeps the configuration it had.
#[tokio::test]
async fn an_update_onto_an_unusable_key_is_refused_and_changes_nothing() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let machine = string(
        &chain
            .sfn("CreateStateMachine", &create_machine_request("updated", None))
            .await,
        "stateMachineArn",
    );
    chain.kms("DisableKey", &json!({ "KeyId": key })).await;

    let request = json!({
        "stateMachineArn": machine,
        "encryptionConfiguration": customer_managed(&key)
    });
    let (status, answer) = chain.call(&SFN, "UpdateStateMachine", &request).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (REFUSAL_STATUS, INVALID_ENCRYPTION_CONFIGURATION),
        "case the update: an update onto an unusable key is refused, got {status} with {answer:?}"
    );
    let described = chain
        .sfn("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        described["encryptionConfiguration"]["type"]
            .as_str()
            .unwrap_or_default(),
        "AWS_OWNED_KEY",
        "case the update: the refused update left the configuration as it was, got {described:?}"
    );

    // And the same update is served once the key is usable again.
    chain.kms("EnableKey", &json!({ "KeyId": key })).await;
    let (status, answer) = chain.call(&SFN, "UpdateStateMachine", &request).await;
    assert_eq!(
        status, 200,
        "case the update: a usable key serves the same update, got {status} with {answer:?}"
    );
}

/// A configuration is applied only while KMS lets the CALLER mint a data key with the
/// key, so a key whose policy allows the caller nothing refuses the create with 400
/// `KmsAccessDeniedException` — naming the principal, the operation and the key — and
/// creates nothing; the same request is served as soon as the statement is back.
///
/// The rows span both kinds of caller: the account's own published credential, which no
/// identity policy governs, and a bounded IAM user carrying the Step Functions create
/// and nothing of KMS.
#[tokio::test]
async fn a_configuration_the_key_policy_refuses_the_caller_creates_nothing() {
    let chain = Chain::new();
    let key = chain.create_key(&withdrawn_policy()).await;
    let (bounded_key, bounded_arn) = chain
        .iam_user(
            "sfn-key-configurer",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    // The create hands the service a role, which is the caller's own
                    // `iam:PassRole`: without it the request is refused before the key
                    // is reached at all, which is a different decision from this one.
                    "Action": ["states:*", "iam:PassRole"],
                    "Resource": "*"
                }]
            }),
        )
        .await;

    // Each row: the access key id the request is signed with, and the principal the
    // refusal must name.
    let callers: Vec<(&str, Option<&str>, String)> = vec![
        ("the account's own credential", None, String::new()),
        (
            "an IAM user with no KMS grant",
            Some(bounded_key.as_str()),
            bounded_arn,
        ),
    ];

    for (index, (case, access_key, principal)) in callers.into_iter().enumerate() {
        let name = format!("denied-machine-{index}");
        let request = create_machine_request(&name, Some(customer_managed(&key)));
        let (status, answer) = match access_key {
            Some(access_key) => {
                chain
                    .call_as(access_key, &SFN, "CreateStateMachine", &request)
                    .await
            }
            None => chain.call(&SFN, "CreateStateMachine", &request).await,
        };
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: a create the key refuses the caller is refused, got {status} with \
             {answer:?}"
        );
        let refusal = message(&answer);
        assert!(
            refusal.contains("kms:DescribeKey") && refusal.contains(&key),
            "case {case:?}: expected the refusal to name the read of the key a configuration \
             begins with and the key itself, got {answer:?}"
        );
        assert!(
            principal.is_empty() || refusal.contains(&principal),
            "case {case:?}: expected the refusal to name {principal:?}, got {answer:?}"
        );
        assert!(
            !is_listed(&chain, "ListStateMachines", "stateMachines", &name).await,
            "case {case:?}: a refused create creates nothing, but {name:?} is listed"
        );
    }

    // The statement back, and the very same create is served.
    chain.put_policy(&key, &granting_policy()).await;
    let request = create_machine_request("denied-machine-0", Some(customer_managed(&key)));
    let (status, answer) = chain.call(&SFN, "CreateStateMachine", &request).await;
    assert_eq!(
        status, 200,
        "case the restored statement: the same create is served, got {status} with {answer:?}"
    );
}

/// A grant conditioned on the ENCRYPTION CONTEXT Step Functions binds its cryptographic
/// operations to scopes the key to one resource: the create of the resource the
/// condition names is served, and an otherwise identical create of another resource is
/// refused `KmsAccessDeniedException`.
///
/// The rows span both resources a key is configured on and the context entry each one
/// is bound to.
#[tokio::test]
async fn an_encryption_context_condition_scopes_the_key_to_one_resource() {
    let chain = Chain::new();

    // Each row: the entry a condition scopes the grant by, the ARN of the resource the
    // condition names, the create of THAT resource, and the create of another one.
    let allowed_machine = "scoped-machine";
    let other_machine = "unscoped-machine";
    let allowed_activity = "scoped-activity";
    let other_activity = "unscoped-activity";
    let rows: Vec<(&str, &str, String, &str, String, &str)> = vec![
        (
            "a state machine's own context",
            STATE_MACHINE_CONTEXT_CONDITION,
            format!("arn:aws:states:us-west-2:{ACCOUNT}:stateMachine:{allowed_machine}"),
            "CreateStateMachine",
            format!("arn:aws:states:us-west-2:{ACCOUNT}:stateMachine:{other_machine}"),
            "ListStateMachines",
        ),
        (
            "an activity's own context",
            ACTIVITY_CONTEXT_CONDITION,
            format!("arn:aws:states:us-west-2:{ACCOUNT}:activity:{allowed_activity}"),
            "CreateActivity",
            format!("arn:aws:states:us-west-2:{ACCOUNT}:activity:{other_activity}"),
            "ListActivities",
        ),
    ];

    for (case, entry, allowed_arn, operation, other_arn, listing) in rows {
        let key = chain
            .create_key(&conditioned_policy(
                &["kms:GenerateDataKey", "kms:Decrypt", "kms:DescribeKey"],
                Some(json!({ "StringEquals": { entry: allowed_arn } })),
            ))
            .await;
        let encryption = customer_managed(&key);
        let (allowed_name, other_name, plural) = if operation == "CreateStateMachine" {
            (allowed_machine, other_machine, "stateMachines")
        } else {
            (allowed_activity, other_activity, "activities")
        };
        let request = |name: &str| {
            if operation == "CreateStateMachine" {
                create_machine_request(name, Some(encryption.clone()))
            } else {
                json!({ "name": name, "encryptionConfiguration": encryption.clone() })
            }
        };

        let (status, answer) = chain.call(&SFN, operation, &request(allowed_name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create the condition names is served, got {status} with \
             {answer:?} (allowed {allowed_arn})"
        );

        let (status, answer) = chain.call(&SFN, operation, &request(other_name)).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: a create the condition does not cover is refused, got {status} with \
             {answer:?} (other {other_arn})"
        );
        assert!(
            !is_listed(&chain, listing, plural, other_name).await,
            "case {case:?}: the refused create created nothing, but {other_name:?} is listed"
        );
    }
}

/// `kmsKeyId` accepts "an alias, alias ARN, key ID, or key ARN of a symmetric
/// encryption KMS key" — four ways of naming ONE key — so whichever form a create used,
/// every read of the resource reports the key's own ARN.
#[tokio::test]
async fn every_reference_form_is_reported_as_the_keys_arn() {
    let chain = Chain::new();
    let key_arn = chain.create_key(&granting_policy()).await;
    let key_id = key_arn
        .split_once("key/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("case the key: expected a key ARN, got {key_arn:?}"));
    let alias = "alias/configured-key";
    chain
        .kms(
            "CreateAlias",
            &json!({ "AliasName": alias, "TargetKeyId": key_id }),
        )
        .await;
    let alias_arn = format!("arn:aws:kms:us-west-2:{ACCOUNT}:{alias}");

    // Rows differ along the reference form `kmsKeyId` carried.
    let rows: Vec<(&str, String)> = vec![
        ("the key ARN", key_arn.clone()),
        ("the bare key id", key_id.clone()),
        ("an alias name", alias.to_string()),
        ("an alias ARN", alias_arn),
    ];

    for (index, (case, reference)) in rows.into_iter().enumerate() {
        let encryption = json!({
            "type": "CUSTOMER_MANAGED_KMS_KEY",
            "kmsKeyId": reference,
            "kmsDataKeyReusePeriodSeconds": 300
        });
        let machine_name = format!("reference-machine-{index}");
        let machine = string(
            &chain
                .sfn(
                    "CreateStateMachine",
                    &create_machine_request(&machine_name, Some(encryption.clone())),
                )
                .await,
            "stateMachineArn",
        );
        let described = chain
            .sfn("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
            .await;
        assert_eq!(
            string(&described["encryptionConfiguration"], "kmsKeyId"),
            key_arn,
            "case {case:?}: DescribeStateMachine reports the key's ARN, got {described:?}"
        );

        let activity_name = format!("reference-activity-{index}");
        let activity = string(
            &chain
                .sfn(
                    "CreateActivity",
                    &json!({ "name": activity_name, "encryptionConfiguration": encryption }),
                )
                .await,
            "activityArn",
        );
        let described = chain
            .sfn("DescribeActivity", &json!({ "activityArn": activity }))
            .await;
        assert_eq!(
            string(&described["encryptionConfiguration"], "kmsKeyId"),
            key_arn,
            "case {case:?}: DescribeActivity reports the key's ARN, got {described:?}"
        );
    }
}

/// An activity with `encryption`, a state machine whose one `Task` state polls it, and
/// one execution of that state machine: the records a delivery below is claimed from.
async fn queued_task(chain: &Chain, name: &str, encryption: Option<Value>) -> String {
    let mut request = json!({ "name": format!("{name}-activity") });
    if let Some(encryption) = encryption {
        request["encryptionConfiguration"] = encryption;
    }
    let activity = string(&chain.sfn("CreateActivity", &request).await, "activityArn");
    let machine = string(
        &chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": activity_definition(&activity, json!({ "TimeoutSeconds": 300 })),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await,
        "stateMachineArn",
    );
    chain
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "input": PAYLOAD }),
        )
        .await;
    activity
}

/// A DELIVERY hands the worker the task's input, which the ACTIVITY's own key sealed, so
/// it follows that key: refused with `KmsInvalidStateException` while the key's state
/// admits no use of it, refused with `KmsAccessDeniedException` while the policy allows
/// the worker nothing, and — the task still queued, because a refusal claims nothing —
/// served as soon as the key answers for it again.
#[tokio::test]
async fn a_delivery_follows_the_activitys_own_key() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let activity = queued_task(&chain, "delivery", Some(customer_managed(&key))).await;
    let poll = json!({ "activityArn": activity, "workerName": WORKER });

    // Rows differ along WHAT makes the key unusable for this delivery: its own key
    // state, and the policy in force over the worker.
    chain.kms("DisableKey", &json!({ "KeyId": key })).await;
    let (status, answer) = chain.call(&SFN, "GetActivityTask", &poll).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (REFUSAL_STATUS, KMS_INVALID_STATE),
        "case the key is disabled: the delivery is refused, got {status} with {answer:?}"
    );
    assert_eq!(
        answer.get("kmsKeyState").and_then(Value::as_str),
        Some("DISABLED"),
        "case the key is disabled: the refusal reports the key state to change, got {answer:?}"
    );

    chain.kms("EnableKey", &json!({ "KeyId": key })).await;
    chain.put_policy(&key, &withdrawn_policy()).await;
    let (status, answer) = chain.call(&SFN, "GetActivityTask", &poll).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
        "case the policy allows the worker nothing: the delivery is refused, got {status} with \
         {answer:?}"
    );
    assert!(
        message(&answer).contains("kms:Decrypt") && message(&answer).contains(&key),
        "case the policy allows the worker nothing: expected the refusal to name the decrypt and \
         the key, got {answer:?}"
    );

    // Neither refusal claimed the task: the same poll is handed it once the key answers
    // for the worker again, with the input the execution really scheduled.
    chain.put_policy(&key, &granting_policy()).await;
    let answer = chain.sfn("GetActivityTask", &poll).await;
    let token = string(&answer, "taskToken");
    assert!(
        !token.is_empty() && string(&answer, "input") == PAYLOAD,
        "case the key answers again: expected the queued task with its own input, got {answer:?}"
    );

    // And a reply to that task is sealed under the same key, so it follows it too.
    let reply = json!({ "taskToken": token, "output": r#"{"done":true}"# });
    chain.put_policy(&key, &withdrawn_policy()).await;
    let (status, answer) = chain.call(&SFN, "SendTaskSuccess", &reply).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
        "case the reply: a reply the activity's key refuses the caller is refused, got {status} \
         with {answer:?}"
    );
    chain.put_policy(&key, &granting_policy()).await;
    let (status, answer) = chain.call(&SFN, "SendTaskSuccess", &reply).await;
    assert_eq!(
        status, 200,
        "case the reply: the restored statement serves the same reply, got {status} with \
         {answer:?}"
    );
}

/// An activity on an Amazon Web Services owned key follows no customer managed key's
/// state, so a delivery is served while a key the account disabled sits beside it.
#[tokio::test]
async fn a_delivery_of_an_unencrypted_activity_follows_no_key() {
    let chain = Chain::new();
    let key = chain.create_key(&withdrawn_policy()).await;
    chain.kms("DisableKey", &json!({ "KeyId": key })).await;

    // Rows differ along HOW the activity ends up on the AWS owned key.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no encryption configuration", None),
        (
            "the AWS owned key named",
            Some(json!({ "type": "AWS_OWNED_KEY" })),
        ),
    ];
    for (index, (case, encryption)) in rows.into_iter().enumerate() {
        let activity = queued_task(&chain, &format!("owned-delivery-{index}"), encryption).await;
        let answer = chain
            .sfn(
                "GetActivityTask",
                &json!({ "activityArn": activity, "workerName": WORKER }),
            )
            .await;
        assert!(
            !string(&answer, "taskToken").is_empty(),
            "case {case:?}: a delivery of an unencrypted activity is served, got {answer:?}"
        );
    }
}

/// `includedData = METADATA_ONLY` reports the execution's metadata and NONE of the
/// sensitive data the key seals, so it makes no call on the key: it keeps answering
/// while the key is in a key state no cryptographic operation can use and while the
/// policy in force denies the caller that key, and only `ALL_DATA` is refused.
#[tokio::test]
async fn a_metadata_only_read_elides_the_data_and_needs_no_key() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let (_machine, execution) = chain
        .run_with_input("metadata-only", Some(customer_managed(&key)), PAYLOAD)
        .await;
    let metadata = json!({ "executionArn": execution, "includedData": "METADATA_ONLY" });
    let all_data = json!({ "executionArn": execution, "includedData": "ALL_DATA" });

    // The members a metadata-only read must not report, and the ones it must.
    let elided = ["input", "inputDetails", "output", "outputDetails", "error", "cause"];
    let kept = ["executionArn", "stateMachineArn", "name", "status", "startDate"];

    let described = chain.sfn("DescribeExecution", &metadata).await;
    for member in elided {
        assert!(
            described.get(member).is_none(),
            "case the key is healthy: a metadata-only read reports no {member:?}, got \
             {described:?}"
        );
    }
    for member in kept {
        assert!(
            described.get(member).is_some(),
            "case the key is healthy: a metadata-only read still reports {member:?}, got \
             {described:?}"
        );
    }
    let status = string(&described, "status");
    // The whole-data read of the same execution does report the sealed data.
    let whole = chain.sfn("DescribeExecution", &all_data).await;
    assert_eq!(
        string(&whole, "input"),
        PAYLOAD,
        "case the key is healthy: a whole-data read reports the input, got {whole:?}"
    );

    // Each row: what makes the key unusable for a read, and the class `ALL_DATA` is
    // refused with while it lasts.
    type KeyStateRow = (&'static str, Vec<(&'static str, Value)>, &'static str);
    let rows: Vec<KeyStateRow> = vec![
        (
            "the key is disabled",
            vec![("DisableKey", json!({ "KeyId": key }))],
            KMS_INVALID_STATE,
        ),
        (
            "the key is enabled again",
            vec![("EnableKey", json!({ "KeyId": key }))],
            "",
        ),
    ];
    for (case, calls, refused_with) in rows {
        drive(&chain, &calls).await;
        let described = chain.sfn("DescribeExecution", &metadata).await;
        assert_eq!(
            string(&described, "status"),
            status,
            "case {case:?}: the metadata-only read keeps answering the same status, got \
             {described:?}"
        );
        for member in elided {
            assert!(
                described.get(member).is_none(),
                "case {case:?}: a metadata-only read reports no {member:?}, got {described:?}"
            );
        }
        let (code, answer) = chain.call(&SFN, "DescribeExecution", &all_data).await;
        if refused_with.is_empty() {
            assert_eq!(
                code, 200,
                "case {case:?}: a whole-data read is served again, got {code} with {answer:?}"
            );
            continue;
        }
        assert_eq!(
            (code, error_code(&answer).as_str()),
            (REFUSAL_STATUS, refused_with),
            "case {case:?}: only the whole-data read is refused, got {code} with {answer:?}"
        );
    }

    // And the same split for the POLICY in force: the metadata-only read is served
    // while the caller holds no grant at all, the whole-data read is not.
    chain.put_policy(&key, &withdrawn_policy()).await;
    let described = chain.sfn("DescribeExecution", &metadata).await;
    assert_eq!(
        string(&described, "status"),
        status,
        "case the policy allows the caller nothing: the metadata-only read is served, got \
         {described:?}"
    );
    let (code, answer) = chain.call(&SFN, "DescribeExecution", &all_data).await;
    assert_eq!(
        (code, error_code(&answer).as_str()),
        (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
        "case the policy allows the caller nothing: the whole-data read is refused, got {code} \
         with {answer:?}"
    );
}

/// A synchronous start seals the run's `input` under the state machine's key and hands
/// the run's data straight back, so it needs the data key for the seal and the decrypt
/// for the data it reports: a caller granted only the mint is served a METADATA-ONLY
/// call — which reports none of that data — and refused the whole-data call.
#[tokio::test]
async fn a_synchronous_start_needs_the_decrypt_only_for_the_data_it_reports() {
    let chain = Chain::new();
    let key = chain
        .create_key(&conditioned_policy(
            &["kms:DescribeKey", "kms:GenerateDataKey"],
            None,
        ))
        .await;
    let machine = string(
        &chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": "sync-cmk",
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "type": "EXPRESS",
                    "encryptionConfiguration": customer_managed(&key),
                }),
            )
            .await,
        "stateMachineArn",
    );

    let metadata_only = json!({
        "stateMachineArn": machine,
        "input": PAYLOAD,
        "includedData": "METADATA_ONLY"
    });
    let answer = chain.sfn("StartSyncExecution", &metadata_only).await;
    for member in ["input", "inputDetails", "output", "outputDetails", "error", "cause"] {
        assert!(
            answer.get(member).is_none(),
            "case metadata only: the response reports no {member:?}, got {answer:?}"
        );
    }
    assert_eq!(
        string(&answer, "status"),
        "SUCCEEDED",
        "case metadata only: the run itself is reported, got {answer:?}"
    );

    let whole_data = json!({ "stateMachineArn": machine, "input": PAYLOAD });
    let (status, answer) = chain.call(&SFN, "StartSyncExecution", &whole_data).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
        "case whole data: a caller granted no decrypt is refused the data, got {status} with \
         {answer:?}"
    );
    assert!(
        message(&answer).contains("kms:Decrypt"),
        "case whole data: expected the refusal to name the decrypt, got {answer:?}"
    );

    // The decrypt granted, and the whole-data call reports the run's own data.
    chain
        .put_policy(
            &key,
            &conditioned_policy(
                &["kms:DescribeKey", "kms:GenerateDataKey", "kms:Decrypt"],
                None,
            ),
        )
        .await;
    let answer = chain.sfn("StartSyncExecution", &whole_data).await;
    assert_eq!(
        string(&answer, "input"),
        PAYLOAD,
        "case the decrypt granted: the whole-data call reports the input, got {answer:?}"
    );

    // And the key's own state refuses both, because the run's data has to be sealed
    // whichever projection the caller asks for.
    chain.kms("DisableKey", &json!({ "KeyId": key })).await;
    for (case, request) in [
        ("metadata only", &metadata_only),
        ("whole data", &whole_data),
    ] {
        let (status, answer) = chain.call(&SFN, "StartSyncExecution", request).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (REFUSAL_STATUS, KMS_INVALID_STATE),
            "case {case:?} with the key disabled: the start is refused, got {status} with \
             {answer:?}"
        );
    }
}

/// A start of a STANDARD execution seals the caller's `input` under the state machine's
/// key, so a caller the key policy allows nothing is refused and nothing is started.
#[tokio::test]
async fn a_start_the_key_refuses_the_caller_starts_nothing() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let machine = string(
        &chain
            .sfn(
                "CreateStateMachine",
                &create_machine_request("start-denied", Some(customer_managed(&key))),
            )
            .await,
        "stateMachineArn",
    );
    chain.put_policy(&key, &withdrawn_policy()).await;

    let (status, answer) = chain
        .call(
            &SFN,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "refused", "input": PAYLOAD }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
        "case the policy allows the caller nothing: the start is refused, got {status} with \
         {answer:?}"
    );
    assert!(
        message(&answer).contains("kms:GenerateDataKey"),
        "case the policy allows the caller nothing: expected the refusal to name the data-key mint \
         sealing the start's input makes, got {answer:?}"
    );
    let listed = chain
        .sfn("ListExecutions", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        listed["executions"].as_array().map(Vec::len),
        Some(0),
        "case the policy allows the caller nothing: a refused start starts nothing, got \
         {listed:?}"
    );

    chain.put_policy(&key, &granting_policy()).await;
    let (status, answer) = chain
        .call(
            &SFN,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "refused", "input": PAYLOAD }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the restored statement: the same start is served, got {status} with {answer:?}"
    );
}

/// An execution does its remaining work under its state machine's key, so the outcome
/// the clock makes due depends on that key at the instant it is owed: one that comes due
/// while the key admits no cryptographic use of it, or while the authorities in force no
/// longer let the EXECUTION ROLE use it, is a failure carrying the language's own runtime
/// error — and nothing is latched, so a redrive with the key answering again finishes the
/// run.
#[tokio::test]
async fn an_execution_whose_key_stops_answering_when_its_outcome_is_due_fails() {
    // Each row: what stops the key answering for the execution's own work before the
    // outcome comes due, and the status the clock then makes due.
    let rows: Vec<(&str, bool, bool, &str)> = vec![
        ("the key keeps answering", false, false, SUCCEEDED),
        ("the key is disabled", true, false, FAILED),
        ("the role's grant is withdrawn", false, true, FAILED),
    ];

    for (index, (case, disable, withdraw, expected)) in rows.into_iter().enumerate() {
        let chain = Chain::new();
        // The role does the execution's own cryptographic work, and the key policy
        // delegates to the account, so the role's own documents are what allows it —
        // which is what withdrawing the key's statement takes away.
        let role_arn = role(
            &chain,
            &format!("due-outcome-role-{index}"),
            &data_key_policy(),
        )
        .await;
        let key = chain.create_key(&granting_policy()).await;
        let machine = string(
            &chain
                .sfn(
                    "CreateStateMachine",
                    &json!({
                        "name": format!("due-outcome-{index}"),
                        "definition": WAITING_DEFINITION,
                        "roleArn": role_arn,
                        "encryptionConfiguration": customer_managed(&key),
                    }),
                )
                .await,
            "stateMachineArn",
        );
        let execution = string(
            &chain
                .sfn(
                    "StartExecution",
                    &json!({ "stateMachineArn": machine, "input": PAYLOAD }),
                )
                .await,
            "executionArn",
        );

        // While the execution is still owed its outcome, the key stops answering for
        // the work it has left to do.
        if disable {
            chain.kms("DisableKey", &json!({ "KeyId": key })).await;
        }
        if withdraw {
            chain.put_policy(&key, &withdrawn_policy()).await;
        }
        std::thread::sleep(PAST_THE_WAIT);

        let metadata = json!({ "executionArn": execution, "includedData": "METADATA_ONLY" });
        let all_data = json!({ "executionArn": execution, "includedData": "ALL_DATA" });
        // The FIRST read of the execution is one the key refuses, and the outcome the
        // clock owed is materialised all the same: it was owed at that instant whether
        // or not the key lets this caller read what it sealed.
        if expected == FAILED {
            let (status, answer) = chain.call(&SFN, "DescribeExecution", &all_data).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: a whole-data read is refused while the key does not answer, got \
                 {status} with {answer:?}"
            );
        }

        // The status is read through the projection that needs no key at all, which is
        // the one an operator still has while a key is unavailable.
        let described = chain.sfn("DescribeExecution", &metadata).await;
        assert_eq!(
            string(&described, "status"),
            expected,
            "case {case:?}: the outcome the clock made due, got {described:?}"
        );
        if expected != FAILED {
            continue;
        }

        // What the failure says: the language's own runtime error, with the refusal
        // about the key as its cause. Read with the key answering again, since the whole
        // data is what the key seals.
        if disable {
            chain.kms("EnableKey", &json!({ "KeyId": key })).await;
        }
        if withdraw {
            chain.put_policy(&key, &granting_policy()).await;
        }
        let whole = chain.sfn("DescribeExecution", &all_data).await;
        assert_eq!(
            string(&whole, "error"),
            RUNTIME_ERROR,
            "case {case:?}: expected the runtime error, got {whole:?}"
        );
        assert!(
            string(&whole, "cause").contains(&key),
            "case {case:?}: expected the cause to relay the refusal about the key, got {whole:?}"
        );

        // And the failure is not latched: with the key answering again, a REDRIVE runs
        // the execution to the outcome its own work computes.
        chain
            .sfn("RedriveExecution", &json!({ "executionArn": execution }))
            .await;
        std::thread::sleep(PAST_THE_WAIT);
        let redriven = chain.sfn("DescribeExecution", &metadata).await;
        assert_eq!(
            string(&redriven, "status"),
            SUCCEEDED,
            "case {case:?}: the key answering again lets the redrive finish, got {redriven:?}"
        );
    }
}

/// A `Task` state that starts an execution of ANOTHER state machine hands that state
/// machine data its OWN key seals, and Step Functions makes the call as this execution's
/// role — so a role the TARGET's key does not permit fails the task instead of starting
/// work whose data cannot be recorded, while the same parent runs when the key permits
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_starting_an_encrypted_child_follows_the_childs_key() {
    // Each row: whether the child's key policy leaves the parent's role the grant the
    // nested start needs, and the status the parent execution reaches.
    let rows: Vec<(&str, bool, &str)> = vec![
        ("the child's key permits the parent's role", true, SUCCEEDED),
        ("the child's key permits it nothing", false, FAILED),
    ];

    for (index, (case, permitted, expected)) in rows.into_iter().enumerate() {
        let chain = Chain::new();
        // The role may start the child — the IAM half of the decision — so what is
        // under test is the key alone.
        let role_arn = role(
            &chain,
            &format!("nested-role-{index}"),
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": [
                        "states:StartExecution",
                        "kms:DescribeKey",
                        "kms:GenerateDataKey",
                        "kms:Decrypt",
                    ],
                    "Resource": "*"
                }]
            }),
        )
        .await;
        let key = chain.create_key(&granting_policy()).await;
        let child = string(
            &chain
                .sfn(
                    "CreateStateMachine",
                    &json!({
                        "name": format!("nested-child-{index}"),
                        "definition": HELLO_DEFINITION,
                        "roleArn": role_arn,
                        "encryptionConfiguration": customer_managed(&key),
                    }),
                )
                .await,
            "stateMachineArn",
        );
        let parent = string(
            &chain
                .sfn(
                    "CreateStateMachine",
                    &json!({
                        "name": format!("nested-parent-{index}"),
                        "definition": nested_start_definition(&child),
                        "roleArn": role_arn,
                    }),
                )
                .await,
            "stateMachineArn",
        );
        if !permitted {
            chain.put_policy(&key, &withdrawn_policy()).await;
        }

        let execution = string(
            &chain
                .sfn(
                    "StartExecution",
                    &json!({ "stateMachineArn": parent, "input": "{}" }),
                )
                .await,
            "executionArn",
        );
        std::thread::sleep(PAST_THE_WAIT);
        let described = chain
            .sfn("DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_eq!(
            string(&described, "status"),
            expected,
            "case {case:?}: the parent execution's outcome, got {described:?}"
        );
        // A refused nested start started nothing of the child's.
        let children = chain
            .sfn("ListExecutions", &json!({ "stateMachineArn": child }))
            .await;
        let started = children["executions"].as_array().map_or(0, Vec::len);
        assert_eq!(
            started,
            usize::from(permitted),
            "case {case:?}: the child's executions, got {children:?}"
        );
    }
}
