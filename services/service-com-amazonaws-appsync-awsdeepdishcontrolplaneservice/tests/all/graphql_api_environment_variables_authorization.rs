// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCE the identity-based decision in front of this service is taken about, and
//! which one its refusal names, over the GraphQL API control plane end to end.
//!
//! The rule under test: a request is judged against the resource its own URI addresses,
//! spelled as the resource portion of an ARN of this service — the ARN spelling this
//! service publishes where the addressed resource has one (an API: `apis/{apiId}`), and the
//! request's own URI path, version prefix and leading `/` included, where it addresses a
//! sub-resource no ARN spelling covers (an API's environment variables). That resource is
//! both what a policy has to state to lift the refusal and what IAM's refusal sentence
//! names, so the two never disagree.
//!
//! The operations this is asserted over are the GraphQL API's own reads and mutations and
//! its environment-variable pair: `GetGraphqlApi` (`GET /v1/apis/{apiId}`),
//! `UpdateGraphqlApi` (`POST /v1/apis/{apiId}`), `DeleteGraphqlApi`
//! (`DELETE /v1/apis/{apiId}`), `GetGraphqlApiEnvironmentVariables`
//! (`GET /v1/apis/{apiId}/environmentVariables`) and `PutGraphqlApiEnvironmentVariables`
//! (`PUT /v1/apis/{apiId}/environmentVariables`), each modelling `AccessDeniedException` at
//! HTTP 403 (`smithy_query(query="operation", operation="GetGraphqlApi,UpdateGraphqlApi,\
//! DeleteGraphqlApi,GetGraphqlApiEnvironmentVariables,PutGraphqlApiEnvironmentVariables")`).
//!
//! The tables span what the rule generalises over: WHICH resource form the request
//! addresses (an API, and the environment-variable sub-resource under one), the METHOD and
//! action it asks for (a read, an update, a delete, a write of the sub-resource), whether
//! the account HOLDS the API named (so the decision is seen to precede the lookup), the
//! KIND of principal (an IAM user, an assumed-role session) and WHAT its documents say
//! (nothing, another action, the OTHER resource spelling of the same call, an explicit deny
//! beside a wildcard allow).
//!
//! The principals are made with IAM's and STS's own operations over the same engine, so the
//! documents this service reads are the ones those services wrote; the harness is the
//! identity-authorization suite's.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::service::authorization::
    ACTION_PREFIX;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;

use crate::identity_authorization::{
    allow, allow_all_but, arn, default_deny_message, Emulator, Principal, Reply, ACCESS_DENIED,
    ACCESS_DENIED_STATUS, ACCOUNT_KEY,
};

/// The name the one API the tables address is created under, which `UpdateGraphqlApi` also
/// re-states (its `name` member is `@required`).
const API_NAME: &str = "Environment_variables_api";

/// An API id of the modelled length (`state::API_ID_LEN`) that nothing created, so a call
/// naming it is a not-found rather than a malformed request.
const ABSENT_API_ID: &str = "zzzzzzzzzzzzzzzzzzzzzzzzzz";

/// The variable the authorized caller stores before the refused callers run, and which must
/// still be the whole list afterwards.
const STORED_KEY: &str = "PROBE";
const STORED_VALUE: &str = "1";

/// The variable a refused write tries to add, which must never reach the store.
const REFUSED_KEY: &str = "FOO";
const REFUSED_VALUE: &str = "bar";

/// The environment-variable sub-resource's own URI path under an API, which is the resource
/// a call addressing it is judged by.
///
/// Source: `smithy_query(query="operation",
/// operation="GetGraphqlApiEnvironmentVariables,PutGraphqlApiEnvironmentVariables")` — both
/// are bound to `/v1/apis/{apiId}/environmentVariables`.
fn environment_variables_path(api_id: &str) -> String {
    format!("/v1/apis/{api_id}/environmentVariables")
}

/// One gated request: the call itself, the action IAM judges it as, the resource it is
/// judged against, the OTHER resource spelling of this service that must NOT serve it, and
/// the answer an authorized caller gets.
struct Probe {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
    other_resource: String,
    served_status: u16,
}

/// The table, one row per resource form and method the rule spans.
///
/// The row that DELETES addresses an API the account does not hold, so the fixture survives
/// the positive control and every row is replayable in any order.
fn probes(api_id: &str) -> Vec<Probe> {
    let api = arn(&state::api_resource(api_id));
    let absent_api = arn(&state::api_resource(ABSENT_API_ID));
    let variables = arn(&environment_variables_path(api_id));
    let absent_variables = arn(&environment_variables_path(ABSENT_API_ID));
    let update = json!({"name": API_NAME, "authenticationType": "API_KEY"});
    let write = json!({"environmentVariables": {REFUSED_KEY: REFUSED_VALUE}});
    vec![
        Probe {
            case: "a read of one GraphQL API",
            operation: "GetGraphqlApi",
            method: "GET",
            path: format!("/v1/apis/{api_id}"),
            body: None,
            resource: api.clone(),
            other_resource: variables.clone(),
            served_status: 200,
        },
        Probe {
            case: "an update of one GraphQL API",
            operation: "UpdateGraphqlApi",
            method: "POST",
            path: format!("/v1/apis/{api_id}"),
            body: Some(update),
            resource: api,
            other_resource: variables.clone(),
            served_status: 200,
        },
        Probe {
            case: "a delete of one GraphQL API the account does not hold",
            operation: "DeleteGraphqlApi",
            method: "DELETE",
            path: format!("/v1/apis/{ABSENT_API_ID}"),
            body: None,
            resource: absent_api.clone(),
            other_resource: absent_variables.clone(),
            // The decision is taken before the lookup, so only an authorized caller learns
            // the API is absent.
            served_status: 404,
        },
        Probe {
            case: "a read of one GraphQL API's environment variables",
            operation: "GetGraphqlApiEnvironmentVariables",
            method: "GET",
            path: environment_variables_path(api_id),
            body: None,
            resource: variables.clone(),
            other_resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Probe {
            case: "a write of one GraphQL API's environment variables",
            operation: "PutGraphqlApiEnvironmentVariables",
            method: "PUT",
            path: environment_variables_path(api_id),
            body: Some(write.clone()),
            resource: variables,
            other_resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Probe {
            case: "a read of the environment variables of an API the account does not hold",
            operation: "GetGraphqlApiEnvironmentVariables",
            method: "GET",
            path: environment_variables_path(ABSENT_API_ID),
            body: None,
            resource: absent_variables.clone(),
            other_resource: absent_api.clone(),
            served_status: 404,
        },
        Probe {
            case: "a write of the environment variables of an API the account does not hold",
            operation: "PutGraphqlApiEnvironmentVariables",
            method: "PUT",
            path: environment_variables_path(ABSENT_API_ID),
            body: Some(write),
            resource: absent_variables,
            other_resource: absent_api,
            served_status: 404,
        },
    ]
}

/// Create the one GraphQL API the tables address and store the one variable they read back,
/// with the account's own credential, and answer its api id.
async fn fixture(emulator: &Emulator) -> String {
    let created = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/apis",
            Some(&json!({"name": API_NAME, "authenticationType": "API_KEY"})),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"the fixture\": expected CreateGraphqlApi 200, got {} {:?} with {:?}",
        created.status, created.error_code, created.body
    );
    let api_id = created.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !api_id.is_empty(),
        "case \"the fixture\": expected the create to answer an apiId, got {:?}",
        created.body
    );
    let stored = emulator
        .call(
            ACCOUNT_KEY,
            "PUT",
            &environment_variables_path(&api_id),
            Some(&json!({"environmentVariables": {STORED_KEY: STORED_VALUE}})),
        )
        .await;
    assert_eq!(
        stored.status, 200,
        "case \"the fixture\": expected PutGraphqlApiEnvironmentVariables 200, got {} {:?} \
         with {:?}",
        stored.status, stored.error_code, stored.body
    );
    api_id
}

/// What a refused caller's documents say, and the clause of IAM's sentence that answers it.
struct Refused {
    case: &'static str,
    /// Built from the row's own action and from the resource spellings of the row.
    document: fn(&Probe) -> Option<String>,
    assumed_role: bool,
    explicit_deny: bool,
}

/// No document at all: IAM's default deny.
fn nothing(_probe: &Probe) -> Option<String> {
    None
}

/// An allow of another action of this service on everything.
fn another_action(_probe: &Probe) -> Option<String> {
    Some(allow(
        &format!("{ACTION_PREFIX}:StartSchemaCreation"),
        "*",
    ))
}

/// An allow of THIS action, stated about the other resource spelling of the same call — the
/// parent API's ARN for a call addressing its environment variables, and that sub-resource's
/// path for a call addressing the API itself.
fn this_action_on_the_other_resource(probe: &Probe) -> Option<String> {
    Some(allow(
        &format!("{ACTION_PREFIX}:{}", probe.operation),
        &probe.other_resource,
    ))
}

/// A wildcard allow over this service with an explicit deny of this action beside it.
fn an_explicit_deny(probe: &Probe) -> Option<String> {
    Some(allow_all_but(&format!(
        "{ACTION_PREFIX}:{}",
        probe.operation
    )))
}

const REFUSALS: &[Refused] = &[
    Refused {
        case: "an IAM user whose documents allow nothing",
        document: nothing,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an assumed-role session whose role allows nothing",
        document: nothing,
        assumed_role: true,
        explicit_deny: false,
    },
    Refused {
        case: "an allow of another action of this service",
        document: another_action,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an allow of this action on the other resource spelling",
        document: this_action_on_the_other_resource,
        assumed_role: false,
        explicit_deny: false,
    },
    Refused {
        case: "an explicit deny beside a wildcard allow",
        document: an_explicit_deny,
        assumed_role: true,
        explicit_deny: true,
    },
];

/// IAM's refusal sentence for `probe`: the principal, the action, the resource the request
/// addressed, and which of IAM's rules withheld the grant.
fn expected_message(principal: &str, probe: &Probe, explicit_deny: bool) -> String {
    if explicit_deny {
        format!(
            "User: {principal} is not authorized to perform: {ACTION_PREFIX}:{} on resource: \
             {} with an explicit deny in an identity-based policy",
            probe.operation, probe.resource
        )
    } else {
        default_deny_message(principal, probe.operation, &probe.resource)
    }
}

/// Assert `reply` is the modelled refusal of `probe`, naming the resource the request
/// addressed and not the other spelling.
fn assert_refused(case: &str, principal: &Principal, probe: &Probe, reply: &Reply, deny: bool) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "case {case:?} / {:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED}, got {} {:?} \
         with {:?}",
        probe.case,
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.message(),
        expected_message(&principal.arn, probe, deny),
        "case {case:?} / {:?}: expected IAM's sentence naming {:?}, got {:?}",
        probe.case,
        probe.resource,
        reply.message()
    );
    assert!(
        !reply.message().contains(&probe.other_resource),
        "case {case:?} / {:?}: expected the refusal to name {:?} rather than {:?}, got {:?}",
        probe.case,
        probe.resource,
        probe.other_resource,
        reply.message()
    );
}

/// The variables the account's own credential reads back for `api_id`.
async fn read_variables(emulator: &Emulator, api_id: &str) -> Value {
    let read = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &environment_variables_path(api_id),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the store afterwards\": expected the variables to be readable, got {} {:?} \
         with {:?}",
        read.status, read.error_code, read.body
    );
    read.body["environmentVariables"].clone()
}

/// RULE: a caller whose identity-based documents do not allow the action on the resource the
/// request addresses is refused with the modelled 403 `AccessDeniedException` naming THAT
/// resource — the API's ARN for a call addressing the API, the sub-resource's own URI path
/// hung off this service's ARN prefix for a call addressing its environment variables — and
/// the call reads and writes nothing.
#[tokio::test]
async fn the_refusal_names_the_resource_the_request_addresses() {
    let emulator = Emulator::new();
    let api_id = fixture(&emulator).await;

    for (index, row) in REFUSALS.iter().enumerate() {
        for (position, probe) in probes(&api_id).into_iter().enumerate() {
            let label = format!("denied_{index}_{position}");
            let document = (row.document)(&probe);
            let principal = if row.assumed_role {
                emulator.assumed_role(&label, document.as_deref()).await
            } else {
                emulator.user(&label, document.as_deref()).await
            };
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    probe.method,
                    &probe.path,
                    probe.body.as_ref(),
                )
                .await;
            assert_refused(row.case, &principal, &probe, &reply, row.explicit_deny);
        }
    }

    // Nothing a refused caller asked for happened: the API is still there and its variables
    // still hold exactly what the authorized caller stored.
    let read = emulator
        .call(ACCOUNT_KEY, "GET", &format!("/v1/apis/{api_id}"), None)
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the API afterwards\": expected the refused delete to have changed nothing, \
         got {} {:?} with {:?}",
        read.status, read.error_code, read.body
    );
    let variables = read_variables(&emulator, &api_id).await;
    assert_eq!(
        variables,
        json!({STORED_KEY: STORED_VALUE}),
        "case \"the variables afterwards\": expected only what the authorized caller stored, \
         got {variables:?}"
    );
}

/// RULE: the resource a policy must state is the SAME one the refusal names — a document
/// allowing the action on it serves the call, and a document allowing the action on the
/// other resource spelling of this service does not, so the API and its environment-variable
/// sub-resource are two distinct resources in both directions.
#[tokio::test]
async fn a_policy_stating_that_resource_serves_the_call() {
    let emulator = Emulator::new();
    let api_id = fixture(&emulator).await;

    for (position, probe) in probes(&api_id).into_iter().enumerate() {
        let granted = emulator
            .user(
                &format!("granted_{position}"),
                Some(&allow(
                    &format!("{ACTION_PREFIX}:{}", probe.operation),
                    &probe.resource,
                )),
            )
            .await;
        let served = emulator
            .call(
                &granted.access_key_id,
                probe.method,
                &probe.path,
                probe.body.as_ref(),
            )
            .await;
        assert_eq!(
            served.status, probe.served_status,
            "case {:?}: expected the operation's own {} answer for a caller the document \
             allows, got {} {:?} with {:?}",
            probe.case, probe.served_status, served.status, served.error_code, served.body
        );
    }
}

/// RULE: the sub-resource's path is read from the REQUEST, so the resource a refusal names
/// tracks the API the request addressed — a second API addressed by the same unauthorized
/// caller is refused about its OWN path, and neither refusal names the other's.
#[tokio::test]
async fn the_sub_resource_path_follows_the_api_the_request_addressed() {
    let emulator = Emulator::new();
    let first = fixture(&emulator).await;
    let second = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/apis",
            Some(&json!({"name": "Second_api", "authenticationType": "API_KEY"})),
        )
        .await;
    let second = second.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !second.is_empty() && second != first,
        "case \"two APIs\": expected a second, distinct API, got {second:?} beside {first:?}"
    );
    let principal = emulator.user("two_api_probe", None).await;

    for (case, api_id, other) in [
        ("the first API", &first, &second),
        ("the second API", &second, &first),
    ] {
        let reply = emulator
            .call(
                &principal.access_key_id,
                "GET",
                &environment_variables_path(api_id),
                None,
            )
            .await;
        let expected = default_deny_message(
            &principal.arn,
            "GetGraphqlApiEnvironmentVariables",
            &arn(&environment_variables_path(api_id)),
        );
        assert_eq!(
            reply.message(),
            expected,
            "case {case:?}: expected the refusal to name this API's own sub-resource path, \
             got {:?}",
            reply.message()
        );
        assert!(
            !reply.message().contains(other.as_str()),
            "case {case:?}: expected the refusal to name nothing of the other API, got {:?}",
            reply.message()
        );
    }
}
