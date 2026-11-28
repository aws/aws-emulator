// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's two POLICY slices: the resource-based policy
//! (`PutResourcePolicy`, `GetResourcePolicy`, `DeleteResourcePolicy`) and the auth policy
//! (`PutAuthPolicy`, `GetAuthPolicy`, `DeleteAuthPolicy`), each driven through the router
//! the way a client drives it and each read back out of a LATER request, so nothing here
//! can pass on a canned body.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, …) is the one the service-slice suite
//! beside this file already builds; nothing is duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. A resource-based policy belongs to the RESOURCE its `resourceArn` names: the
//!    document a put submits is the document a later get reports, verbatim; a second put
//!    replaces it; a change to anything else — the resource's own members, another
//!    resource's policy — leaves it exactly as it was; and the delete removes it, after
//!    which the get reports no policy at all.
//! 2. The policy operations resolve the resource their ARN names before touching a
//!    policy: an ARN of the modelled shape naming no persisted resource is a 404
//!    `ResourceNotFoundException` built over the kind the ARN named, and an input outside
//!    a modelled constraint is a 400 — in neither case is anything stored.
//!
//! The table spans what rule 1 generalises over: the KIND the policy is attached to
//! (service network and service, the kinds this emulator can hold), and the FORM of the
//! document (statement count, padding whitespace, non-ASCII characters, and a document
//! close to the `PolicyString` ceiling). The recorded case — one document on one service
//! network, surviving an unrelated update and then cleared — is one row of it.
//!
//! The auth policy is the OTHER document those same resources carry, so its rules are
//! asserted here too, over tables of the same shape:
//!
//! 3. An auth policy belongs to the resource its `resourceIdentifier` names, under either
//!    modelled form of that identifier (the bare id or the resource's ARN): the document a
//!    put submits is the document a later get reports verbatim, a second put replaces it
//!    and moves only `lastUpdatedAt`, the delete removes it, and the resource's OTHER
//!    policy is never touched.
//! 4. The `state` an auth policy reports is derived from the auth type of the resource it
//!    is attached to, at read time: `Active` only while that auth type is `AWS_IAM`, so
//!    updating the auth type flips what the same stored document reports without changing
//!    the document or its timestamps.
//! 5. A `policy` the service cannot read as a JSON policy document is refused with the
//!    modelled 400 in AWS's words, and the policy the resource already carried is left
//!    exactly as it was; so is an identifier that names no persisted resource (404) and a
//!    value outside a modelled constraint (400).
//!
//! TAGS are the third thing this service attaches to a resource by `resourceArn`, so the
//! tag operations (`TagResource`, `ListTagsForResource`, `UntagResource`) are asserted
//! here too, over tables of the same shape:
//!
//! 6. A tag call addresses the tag map of the resource its `resourceArn` names — the ONE
//!    map that resource's create stored: `TagResource` MERGES the pairs it sends into it
//!    (replacing the keys it names, case-sensitively, and leaving every other key), and
//!    `ListTagsForResource` reports exactly that map, so one resource's tags never change
//!    another's.
//! 7. `UntagResource` removes only the keys it names, and a key the resource does not
//!    carry removes nothing.
//! 8. A `resourceArn` naming no resource a tag call can address is refused the way the
//!    tag front-end refuses one — HTTP 404 with the error code `NotFoundException` and a
//!    sentence that names nothing from the request — for every one of the three
//!    operations, and a write that would take a resource past the modelled `TagMap`
//!    ceiling is a 400 that stores nothing.
//!
//! The tag tables span what rules 6–8 generalise over: the KIND of resource the ARN names
//! (one of every kind this emulator mints, including the two whose ARN NESTS under another
//! resource's), and the tags written (pair count, a replaced key, an empty value,
//! non-ASCII text, and a key and value at their modelled `@length` ceilings).

use std::collections::BTreeMap;

use crate::resource_not_found_messages::absent_id;
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_timestamp, message_of, Answer, Lattice,
};

/// `PUT /resourcepolicy/{resourceArn}`.
async fn put_policy(lattice: &Lattice, resource_arn: &str, policy: &str) -> Answer {
    lattice
        .call(
            "PUT",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            Some(serde_json::json!({ "policy": policy })),
        )
        .await
}

/// `GET /resourcepolicy/{resourceArn}`.
async fn get_policy(lattice: &Lattice, resource_arn: &str) -> Answer {
    lattice
        .call(
            "GET",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            None,
        )
        .await
}

/// `DELETE /resourcepolicy/{resourceArn}`.
async fn delete_policy(lattice: &Lattice, resource_arn: &str) -> Answer {
    lattice
        .call(
            "DELETE",
            &format!("/resourcepolicy/{}", as_label(resource_arn)),
            None,
        )
        .await
}

/// The ARN resource segments of the kinds a `resourceArn` can name, from that member's
/// own modelled pattern (`servicenetwork/sn-`, `service/svc-`,
/// `resourceconfiguration/rcfg-`; `smithy_query(query="shape",shape="ResourceArn")`).
const ARN_KIND_SERVICE_NETWORK: &str = "servicenetwork";
const ARN_KIND_SERVICE: &str = "service";
const ARN_KIND_RESOURCE_CONFIGURATION: &str = "resourceconfiguration";

/// The id prefixes of those same kinds, from the same pattern.
const PREFIX_SERVICE_NETWORK: &str = "sn-";
const PREFIX_SERVICE: &str = "svc-";
const PREFIX_RESOURCE_CONFIGURATION: &str = "rcfg-";

/// The `@length` ceiling the model puts on `PolicyString` (`len 1..10000`).
const POLICY_MAX_LENGTH: usize = 10_000;

/// The policy documents the table spans. No single document is the requirement: the rule
/// is that whatever document the caller submits is the one a later read reports, so the
/// rows differ in statement count, in padding whitespace, in the characters they carry
/// and in length.
fn policy_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"share","Effect":"Allow","Principal":{"AWS":"123456789012"},"Action":"vpc-lattice:CreateServiceNetworkServiceAssociation","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "two statements, one of them conditioned",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":["vpc-lattice:CreateServiceNetworkVpcAssociation"],"Resource":"*","Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-example"}}},{"Effect":"Deny","Principal":"*","Action":"vpc-lattice:DeleteServiceNetwork","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a document padded with whitespace",
            r#"   { "Version": "2012-10-17", "Statement": [ { "Effect": "Allow", "Principal": { "AWS": "210987654321" }, "Action": "vpc-lattice:GetService", "Resource": "*" } ] }   "#
                .to_string(),
        ),
        (
            "non-ASCII characters in a statement id",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-ünïcode","Effect":"Allow","Principal":"*","Action":"vpc-lattice:GetServiceNetwork","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a document just inside the PolicyString ceiling",
            {
                let head = r#"{"Version":"2012-10-17","Id":""#;
                let tail = r#"","Statement":[]}"#;
                let filler = POLICY_MAX_LENGTH - head.len() - tail.len();
                format!("{head}{}{tail}", "a".repeat(filler))
            },
        ),
    ]
}

/// The `policy` member a get reported, which must be the JSON string the put submitted.
fn reported_policy(case: &str, answer: &Answer) -> String {
    answer.body["policy"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must report the stored policy as a string, got {}",
                answer.body
            )
        })
        .to_string()
}

/// A read that reports no policy at all: the member the model declares optional is
/// absent, which is what a resource with no policy attached looks like.
fn assert_no_policy(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a resource with no policy is still a resource, expected 200, \
         got {} / {}",
        answer.status, answer.body
    );
    assert!(
        answer.body["policy"].is_null(),
        "case {case:?}: expected no policy member, got {}",
        answer.body
    );
}

/// Rule 1, over the document forms × the kinds a policy can be attached to here.
///
/// Each row: the put stores the document, the get reports it verbatim, an unrelated
/// change to the resource (and a policy on another resource) leaves it alone, a second
/// put replaces it, and the delete clears it — after which the get reports no policy and
/// a repeated delete is still accepted.
#[tokio::test]
async fn a_resource_policy_round_trips_survives_unrelated_changes_and_clears_on_delete() {
    let lattice = Lattice::new();

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "policy-net" }))
        .await;
    assert_eq!(
        network.status, 201,
        "fixture service network: expected 201, got {} / {}",
        network.status, network.body
    );
    let service = lattice
        .create_service(serde_json::json!({ "name": "policy-svc" }))
        .await;
    assert_eq!(
        service.status, 201,
        "fixture service: expected 201, got {} / {}",
        service.status, service.body
    );
    let network_id = network.str_field("id");
    let network_arn = network.str_field("arn");
    let service_id = service.str_field("id");
    let service_arn = service.str_field("arn");

    // The resource a row attaches its policy to, and the "something else changed" step
    // that must not disturb it: for a service network, an `authType` update of the
    // network itself; for a service, a listener created under it.
    let documents = policy_documents();
    let targets = [
        ("service network", network_arn.clone(), network_id.clone()),
        ("service", service_arn.clone(), service_id.clone()),
    ];

    for (row, (form, document)) in documents.iter().enumerate() {
        for (kind, target_arn, target_id) in &targets {
            let case = format!("{kind}: {form}");

            let put = put_policy(&lattice, target_arn, document).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: the put expected 200, got {} / {}",
                put.status, put.body
            );

            let read = get_policy(&lattice, target_arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the read expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                &reported_policy(&case, &read),
                document,
                "case {case:?}: the stored policy must be the submitted document verbatim"
            );

            // Something else changes: the resource's own members, and the OTHER
            // resource's policy. Neither may touch this one.
            let unrelated = if kind == &"service network" {
                lattice
                    .call(
                        "PATCH",
                        &format!("/servicenetworks/{target_id}"),
                        Some(serde_json::json!({ "authType": "AWS_IAM" })),
                    )
                    .await
            } else {
                lattice
                    .call(
                        "POST",
                        &format!("/services/{target_id}/listeners"),
                        Some(serde_json::json!({
                            "name": format!("policy-listener-{row}"),
                            "protocol": "HTTP",
                            "port": 8000 + i32::try_from(row).expect("the row index fits"),
                            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
                        })),
                    )
                    .await
            };
            assert!(
                (200..300).contains(&unrelated.status),
                "case {case:?}: the unrelated change expected a 2xx, got {} / {}",
                unrelated.status,
                unrelated.body
            );
            let other_arn = if kind == &"service network" {
                &service_arn
            } else {
                &network_arn
            };
            let other_document = format!(r#"{{"Version":"2012-10-17","Id":"other-{row}"}}"#);
            let other_put = put_policy(&lattice, other_arn, &other_document).await;
            assert_eq!(
                other_put.status, 200,
                "case {case:?}: the other resource's put expected 200, got {} / {}",
                other_put.status, other_put.body
            );

            let after = get_policy(&lattice, target_arn).await;
            assert_eq!(
                &reported_policy(&case, &after),
                document,
                "case {case:?}: the policy must survive a change to anything else"
            );
            let other_read = get_policy(&lattice, other_arn).await;
            assert_eq!(
                reported_policy(&case, &other_read),
                other_document,
                "case {case:?}: each resource keeps its OWN policy"
            );

            // A second put REPLACES the document rather than merging or appending.
            let replacement = format!(r#"{{"Version":"2012-10-17","Id":"replacement-{row}"}}"#);
            let replaced = put_policy(&lattice, target_arn, &replacement).await;
            assert_eq!(
                replaced.status, 200,
                "case {case:?}: the replacing put expected 200, got {} / {}",
                replaced.status, replaced.body
            );
            let reread = get_policy(&lattice, target_arn).await;
            assert_eq!(
                reported_policy(&case, &reread),
                replacement,
                "case {case:?}: the latest put is the policy the resource has"
            );

            // The delete clears it, and nothing else.
            let deleted = delete_policy(&lattice, target_arn).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: the delete expected 204, got {} / {}",
                deleted.status, deleted.body
            );
            assert_no_policy(&case, &get_policy(&lattice, target_arn).await);
            assert_eq!(
                reported_policy(&case, &get_policy(&lattice, other_arn).await),
                other_document,
                "case {case:?}: clearing one resource's policy leaves the other's"
            );

            // `DeleteResourcePolicy` is `@idempotent`: a resource that already has no
            // policy is accepted, not refused.
            let again = delete_policy(&lattice, target_arn).await;
            assert_eq!(
                again.status, 204,
                "case {case:?}: a repeated delete expected 204, got {} / {}",
                again.status, again.body
            );
            assert_no_policy(&case, &get_policy(&lattice, target_arn).await);

            // Leave the store as the next row expects it.
            let cleared = delete_policy(&lattice, other_arn).await;
            assert_eq!(
                cleared.status, 204,
                "case {case:?}: clearing the other resource expected 204, got {} / {}",
                cleared.status, cleared.body
            );
        }
    }
}

/// Rule 2's not-founds: an ARN of the modelled shape that names no persisted resource is
/// a 404 on every policy operation, with AWS's not-found construction over the kind the
/// ARN named and the ARN the caller submitted.
///
/// The rows span the three kinds `ResourceArn` enumerates — including the resource
/// configuration, which no implemented create can mint — and all three operations.
#[tokio::test]
async fn a_policy_call_naming_no_persisted_resource_is_the_modelled_not_found() {
    let lattice = Lattice::new();

    // (case, ARN, the CamelCase kind token AWS's message names, the resourceType token).
    // Both tokens are written out here rather than read from the service, so no row can
    // pass by agreeing with the code under test.
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a service network that was never created",
            expected_arn(
                ARN_KIND_SERVICE_NETWORK,
                &absent_id(PREFIX_SERVICE_NETWORK, "0"),
            ),
            "ServiceNetwork",
            "SERVICE_NETWORK",
        ),
        (
            "a service that was never created",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE, "z")),
            "Service",
            "SERVICE",
        ),
        (
            "a resource configuration, a kind nothing can create yet",
            expected_arn(
                ARN_KIND_RESOURCE_CONFIGURATION,
                &absent_id(PREFIX_RESOURCE_CONFIGURATION, "a1b2c3d4"),
            ),
            "ResourceConfiguration",
            "RESOURCE_CONFIGURATION",
        ),
    ];

    for (row, arn, label, resource_type) in rows {
        for (operation, answer) in [
            (
                "PutResourcePolicy",
                put_policy(&lattice, &arn, r#"{"Version":"2012-10-17"}"#).await,
            ),
            ("GetResourcePolicy", get_policy(&lattice, &arn).await),
            ("DeleteResourcePolicy", delete_policy(&lattice, &arn).await),
        ] {
            let case = format!("{operation}: {row}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
                answer.error_code,
                answer.body
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(format!("{label} {arn} cannot be found.").as_str()),
                "case {case:?}: expected AWS's not-found construction, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected the kind's resourceType token, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: resourceId must name what the request named, got {}",
                answer.body
            );
        }
        // A refused put stores nothing, so the resource still has no policy to read.
        let read = get_policy(&lattice, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {row:?}: a refused put must not create a resource, got {} / {}",
            read.status, read.body
        );
    }
}

/// Rule 2's modelled-constraint refusals: a `policy` outside `PolicyString`'s `@length`
/// or `@pattern`, and a `resourceArn` outside `ResourceArn`'s pattern, are 400s — and
/// none of them writes a policy onto the resource the request named.
#[tokio::test]
async fn a_policy_call_outside_a_modelled_constraint_is_400_and_writes_nothing() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "constraint-net" }))
        .await;
    let network_id = network.str_field("id");
    let network_arn = network.str_field("arn");

    let stored = r#"{"Version":"2012-10-17","Id":"kept"}"#;
    let put = put_policy(&lattice, &network_arn, stored).await;
    assert_eq!(
        put.status, 200,
        "fixture put: expected 200, got {} / {}",
        put.status, put.body
    );

    // (case, the ARN the request names, the policy it submits)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a policy shorter than the modelled minimum",
            network_arn.clone(),
            String::new(),
        ),
        (
            "a policy of whitespace only, which the pattern forbids",
            network_arn.clone(),
            "   ".to_string(),
        ),
        (
            "a policy one character past the modelled ceiling",
            network_arn.clone(),
            format!(
                r#"{{"Id":"{}"}}"#,
                "b".repeat(POLICY_MAX_LENGTH - r#"{"Id":""#.len() - r#""}"#.len() + 1)
            ),
        ),
        (
            "the bare resource id, a form the ARN member does not model",
            network_id.clone(),
            stored.to_string(),
        ),
        (
            "an ARN whose kind segment and id prefix disagree",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE_NETWORK, "0")),
            stored.to_string(),
        ),
    ];

    for (case, arn, policy) in rows {
        let answer = put_policy(&lattice, &arn, &policy).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        // The refusal wrote nothing: the policy the fixture put is still the one stored.
        let read = get_policy(&lattice, &network_arn).await;
        assert_eq!(
            reported_policy(case, &read),
            stored,
            "case {case:?}: a refused call must not change the stored policy"
        );
    }
}

// ---------------------------------------------------------------------------
// The auth policy (rules 3, 4 and 5).
// ---------------------------------------------------------------------------

/// `PUT /authpolicy/{resourceIdentifier}`.
async fn put_auth_policy(lattice: &Lattice, identifier: &str, policy: &str) -> Answer {
    lattice
        .call(
            "PUT",
            &format!("/authpolicy/{}", as_label(identifier)),
            Some(serde_json::json!({ "policy": policy })),
        )
        .await
}

/// `GET /authpolicy/{resourceIdentifier}`.
async fn get_auth_policy(lattice: &Lattice, identifier: &str) -> Answer {
    lattice
        .call("GET", &format!("/authpolicy/{}", as_label(identifier)), None)
        .await
}

/// `DELETE /authpolicy/{resourceIdentifier}`.
async fn delete_auth_policy(lattice: &Lattice, identifier: &str) -> Answer {
    lattice
        .call(
            "DELETE",
            &format!("/authpolicy/{}", as_label(identifier)),
            None,
        )
        .await
}

/// The values `AuthPolicyState` admits
/// (`smithy_query(query="shape",shape="AuthPolicyState")`), written out here rather than
/// read from the service, so no row can pass by agreeing with the code under test.
const STATE_ACTIVE: &str = "Active";
const STATE_INACTIVE: &str = "Inactive";

/// The values `AuthType` admits (`smithy_query(query="shape",shape="AuthType")`).
const AUTH_TYPE_AWS_IAM: &str = "AWS_IAM";
const AUTH_TYPE_NONE: &str = "NONE";

/// The `@length` ceiling the model puts on `AuthPolicyString`
/// (`smithy_query(query="shape",shape="AuthPolicyString")`: `len ..102400`).
const AUTH_POLICY_MAX_LENGTH: usize = 102_400;

/// AWS's refusal of a `policy` that is not a policy document at all.
const MALFORMED_AUTH_POLICY_MESSAGE: &str = "Not a valid policy";

/// The state rule 4 demands of a policy attached to a resource whose auth type is
/// `auth_type`: "The auth policy is only active when the auth type is set to AWS_IAM …
/// If the Auth type is NONE, then, any auth policy that you provide remains inactive"
/// (the `state` member's own documentation).
fn expected_state(auth_type: &str) -> &'static str {
    if auth_type == AUTH_TYPE_AWS_IAM {
        STATE_ACTIVE
    } else {
        STATE_INACTIVE
    }
}

/// One resource an auth policy can be attached to here, in both identifier forms the
/// model admits, plus the auth type it was created with.
struct AuthTarget {
    case: &'static str,
    auth_type: &'static str,
    id: String,
    arn: String,
}

/// The resources rule 3 is driven over: a service network and a service, each under an
/// auth type that makes a policy active and one that does not — including a service
/// network created with no `authType` at all, which the model documents as `NONE`.
async fn auth_targets(lattice: &Lattice) -> Vec<AuthTarget> {
    let mut targets = Vec::new();
    for (case, auth_type, name, created) in [
        (
            "a service network created with no authType",
            AUTH_TYPE_NONE,
            "auth-net-default",
            None,
        ),
        (
            "a service network created with IAM auth",
            AUTH_TYPE_AWS_IAM,
            "auth-net-iam",
            Some(AUTH_TYPE_AWS_IAM),
        ),
    ] {
        let mut body = serde_json::json!({ "name": name });
        if let Some(auth) = created {
            body["authType"] = serde_json::json!(auth);
        }
        let answer = lattice.create_service_network(body).await;
        assert_eq!(
            answer.status, 201,
            "fixture {case:?}: expected 201, got {} / {}",
            answer.status, answer.body
        );
        targets.push(AuthTarget {
            case,
            auth_type,
            id: answer.str_field("id"),
            arn: answer.str_field("arn"),
        });
    }
    for (case, auth_type, name) in [
        ("a service created with NONE auth", AUTH_TYPE_NONE, "auth-svc-none"),
        (
            "a service created with IAM auth",
            AUTH_TYPE_AWS_IAM,
            "auth-svc-iam",
        ),
    ] {
        let answer = lattice
            .create_service(serde_json::json!({ "name": name, "authType": auth_type }))
            .await;
        assert_eq!(
            answer.status, 201,
            "fixture {case:?}: expected 201, got {} / {}",
            answer.status, answer.body
        );
        targets.push(AuthTarget {
            case,
            auth_type,
            id: answer.str_field("id"),
            arn: answer.str_field("arn"),
        });
    }
    targets
}

/// The auth-policy documents the table spans. No single document is the requirement: the
/// rule is that whatever document the caller submits is the one a later read reports, so
/// the rows differ in statement count, in padding, in the characters they carry and in
/// length.
fn auth_policy_documents() -> Vec<(&'static str, String)> {
    vec![
        (
            "one statement",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"vpc-lattice-svcs:Invoke","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "two statements, one of them conditioned",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"vpc-lattice-svcs:Invoke","Resource":"*","Condition":{"StringEquals":{"vpc-lattice-svcs:RequestMethod":"GET"}}},{"Effect":"Deny","Principal":"*","Action":"vpc-lattice-svcs:Invoke","Resource":"*","Condition":{"StringNotEquals":{"aws:PrincipalOrgID":"o-example"}}}]}"#
                .to_string(),
        ),
        (
            "a document padded with spaces",
            r#"  { "Version": "2012-10-17", "Statement": [ { "Effect": "Allow", "Principal": "*", "Action": "vpc-lattice-svcs:Invoke", "Resource": "*" } ] }  "#
                .to_string(),
        ),
        (
            "non-ASCII characters in a statement id",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"café-ünïcode","Effect":"Deny","Principal":"*","Action":"vpc-lattice-svcs:Invoke","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a document just inside the AuthPolicyString ceiling",
            {
                let head = r#"{"Version":"2012-10-17","Id":""#;
                let tail = r#"","Statement":[]}"#;
                let filler = AUTH_POLICY_MAX_LENGTH - head.len() - tail.len();
                format!("{head}{}{tail}", "a".repeat(filler))
            },
        ),
    ]
}

/// The instant a timestamp member reports, as epoch seconds, so two reads of the same
/// stored instant can be compared without asserting on the wall clock.
fn instant_of(case: &str, answer: &Answer, member: &str) -> f64 {
    assert!(
        is_timestamp(&answer.body[member]),
        "case {case:?}: {member} must be the modelled RFC 3339 instant, got {}",
        answer.body
    );
    let text = answer.body[member]
        .as_str()
        .expect("the member was just checked to be a timestamp string");
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} must parse as an instant ({error})")
        })
        .as_secs_f64()
}

/// A read that reports the stored auth policy: the document verbatim, the state its
/// resource's auth type gives it, and both instants in the modelled wire form.
fn assert_auth_policy(case: &str, answer: &Answer, document: &str, state: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["policy"].as_str(),
        Some(document),
        "case {case:?}: the read must report the submitted document verbatim, got {}",
        answer.body
    );
    assert_eq!(
        answer.body["state"].as_str(),
        Some(state),
        "case {case:?}: expected state {state}, got {}",
        answer.body
    );
}

/// A read of a resource that carries no auth policy: a 200 reporting none of the members
/// the model declares optional.
fn assert_no_auth_policy(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a resource with no auth policy is still a resource, expected 200, \
         got {} / {}",
        answer.status, answer.body
    );
    for member in ["policy", "state", "createdAt", "lastUpdatedAt"] {
        assert!(
            answer.body[member].is_null(),
            "case {case:?}: expected no {member} member, got {}",
            answer.body
        );
    }
}

/// One refusal of a document the service cannot read as a policy: the modelled 400 in
/// AWS's words.
fn assert_malformed_auth_policy(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        message_of(case, answer),
        MALFORMED_AUTH_POLICY_MESSAGE,
        "case {case:?}: expected AWS's refusal of a document that is not a policy"
    );
}

/// Rule 3, over the document forms × the resources an auth policy can be attached to.
///
/// Each row: the put stores the document under the resource's BARE ID and the read finds
/// it under the resource's ARN (and the replacement the other way round, so both modelled
/// forms resolve the same record); a second put replaces the document and keeps
/// `createdAt`; the delete clears it, after which the read reports no policy and a
/// repeated delete is still accepted.
#[tokio::test]
async fn an_auth_policy_round_trips_under_either_identifier_form_and_clears_on_delete() {
    let lattice = Lattice::new();
    let targets = auth_targets(&lattice).await;

    for (row, (form, document)) in auth_policy_documents().into_iter().enumerate() {
        for target in &targets {
            let case = format!("{}: {form}", target.case);
            let state = expected_state(target.auth_type);

            let put = put_auth_policy(&lattice, &target.id, &document).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: the put expected 200, got {} / {}",
                put.status, put.body
            );
            assert_eq!(
                put.body["policy"].as_str(),
                Some(document.as_str()),
                "case {case:?}: the put must answer with the document it stored, got {}",
                put.body
            );
            assert_eq!(
                put.body["state"].as_str(),
                Some(state),
                "case {case:?}: the put expected state {state}, got {}",
                put.body
            );

            let read = get_auth_policy(&lattice, &target.arn).await;
            assert_auth_policy(&case, &read, &document, state);
            let created = instant_of(&case, &read, "createdAt");
            let updated = instant_of(&case, &read, "lastUpdatedAt");
            assert!(
                updated >= created,
                "case {case:?}: lastUpdatedAt {updated} must not precede createdAt {created}"
            );

            // A second put REPLACES the document rather than merging or appending, and
            // only the update instant moves with it.
            let replacement = format!(r#"{{"Version":"2012-10-17","Id":"replacement-{row}"}}"#);
            let replaced = put_auth_policy(&lattice, &target.arn, &replacement).await;
            assert_eq!(
                replaced.status, 200,
                "case {case:?}: the replacing put expected 200, got {} / {}",
                replaced.status, replaced.body
            );
            let reread = get_auth_policy(&lattice, &target.id).await;
            assert_auth_policy(&case, &reread, &replacement, state);
            assert_eq!(
                instant_of(&case, &reread, "createdAt"),
                created,
                "case {case:?}: a replacement keeps the instant the policy was attached"
            );
            assert!(
                instant_of(&case, &reread, "lastUpdatedAt") >= created,
                "case {case:?}: lastUpdatedAt must not precede createdAt, got {}",
                reread.body
            );

            // The delete clears it, and a repeated delete is accepted: the operation is
            // `@idempotent`.
            let deleted = delete_auth_policy(&lattice, &target.arn).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: the delete expected 204, got {} / {}",
                deleted.status, deleted.body
            );
            assert_no_auth_policy(&case, &get_auth_policy(&lattice, &target.id).await);
            let again = delete_auth_policy(&lattice, &target.id).await;
            assert_eq!(
                again.status, 204,
                "case {case:?}: a repeated delete expected 204, got {} / {}",
                again.status, again.body
            );
            assert_no_auth_policy(&case, &get_auth_policy(&lattice, &target.arn).await);
        }
    }
}

/// Rule 3's separation: the two policies a resource carries are different documents, and
/// neither operation reads or clears the other's.
#[tokio::test]
async fn the_auth_policy_and_the_resource_policy_of_one_resource_are_separate_documents() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(
            serde_json::json!({ "name": "two-policies", "authType": AUTH_TYPE_AWS_IAM }),
        )
        .await;
    assert_eq!(
        network.status, 201,
        "fixture service network: expected 201, got {} / {}",
        network.status, network.body
    );
    let id = network.str_field("id");
    let arn = network.str_field("arn");

    let auth_document = r#"{"Version":"2012-10-17","Id":"auth"}"#;
    let resource_document = r#"{"Version":"2012-10-17","Id":"resource"}"#;
    let case = "a service network carrying both policies";
    assert_eq!(
        put_auth_policy(&lattice, &id, auth_document).await.status,
        200,
        "case {case:?}: the auth-policy put expected 200"
    );
    assert_eq!(
        put_policy(&lattice, &arn, resource_document).await.status,
        200,
        "case {case:?}: the resource-policy put expected 200"
    );

    assert_auth_policy(
        case,
        &get_auth_policy(&lattice, &arn).await,
        auth_document,
        STATE_ACTIVE,
    );
    assert_eq!(
        reported_policy(case, &get_policy(&lattice, &arn).await),
        resource_document,
        "case {case:?}: each policy kind reports its OWN document"
    );

    // Clearing one leaves the other exactly as it was, in both directions.
    assert_eq!(
        delete_auth_policy(&lattice, &id).await.status,
        204,
        "case {case:?}: the auth-policy delete expected 204"
    );
    assert_no_auth_policy(case, &get_auth_policy(&lattice, &id).await);
    assert_eq!(
        reported_policy(case, &get_policy(&lattice, &arn).await),
        resource_document,
        "case {case:?}: clearing the auth policy must leave the resource policy"
    );
    assert_eq!(
        put_auth_policy(&lattice, &arn, auth_document).await.status,
        200,
        "case {case:?}: re-attaching the auth policy expected 200"
    );
    assert_eq!(
        delete_policy(&lattice, &arn).await.status,
        204,
        "case {case:?}: the resource-policy delete expected 204"
    );
    assert_auth_policy(
        case,
        &get_auth_policy(&lattice, &id).await,
        auth_document,
        STATE_ACTIVE,
    );
}

/// Rule 4: the state a read reports follows the auth type of the resource the policy is
/// attached to, at read time.
///
/// Each row starts from one auth type and flips it, so the same stored document reports
/// `Active` and `Inactive` in turn; the document and both of its instants stay put,
/// because updating the resource's auth type is not a change to its policy.
#[tokio::test]
async fn the_auth_policy_state_tracks_the_auth_type_of_its_resource() {
    let lattice = Lattice::new();
    let document = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"vpc-lattice-svcs:Invoke","Resource":"*"}]}"#;

    // (case, the authType the network is created with, the flips applied in order)
    let rows: [(&str, Option<&str>, [&str; 3]); 3] = [
        (
            "created with no authType at all",
            None,
            [AUTH_TYPE_AWS_IAM, AUTH_TYPE_NONE, AUTH_TYPE_AWS_IAM],
        ),
        (
            "created with NONE auth",
            Some(AUTH_TYPE_NONE),
            [AUTH_TYPE_AWS_IAM, AUTH_TYPE_AWS_IAM, AUTH_TYPE_NONE],
        ),
        (
            "created with IAM auth",
            Some(AUTH_TYPE_AWS_IAM),
            [AUTH_TYPE_NONE, AUTH_TYPE_AWS_IAM, AUTH_TYPE_NONE],
        ),
    ];

    for (row, (case, created_with, flips)) in rows.into_iter().enumerate() {
        let mut body = serde_json::json!({ "name": format!("auth-state-{row}") });
        if let Some(auth) = created_with {
            body["authType"] = serde_json::json!(auth);
        }
        let network = lattice.create_service_network(body).await;
        assert_eq!(
            network.status, 201,
            "case {case:?}: the fixture expected 201, got {} / {}",
            network.status, network.body
        );
        let id = network.str_field("id");

        let put = put_auth_policy(&lattice, &id, document).await;
        let state_at_create = expected_state(created_with.unwrap_or(AUTH_TYPE_NONE));
        assert_eq!(
            put.body["state"].as_str(),
            Some(state_at_create),
            "case {case:?}: the put expected state {state_at_create}, got {}",
            put.body
        );
        let first = get_auth_policy(&lattice, &id).await;
        assert_auth_policy(case, &first, document, state_at_create);
        let created = instant_of(case, &first, "createdAt");
        let updated = instant_of(case, &first, "lastUpdatedAt");

        for flip in flips {
            let patched = lattice
                .call(
                    "PATCH",
                    &format!("/servicenetworks/{id}"),
                    Some(serde_json::json!({ "authType": flip })),
                )
                .await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: the authType update expected 200, got {} / {}",
                patched.status, patched.body
            );
            assert_eq!(
                patched.body["authType"].as_str(),
                Some(flip),
                "case {case:?}: the update must report the auth type it stored, got {}",
                patched.body
            );

            let read = get_auth_policy(&lattice, &id).await;
            assert_auth_policy(case, &read, document, expected_state(flip));
            assert_eq!(
                instant_of(case, &read, "createdAt"),
                created,
                "case {case:?}: the auth type is not the policy: createdAt must not move"
            );
            assert_eq!(
                instant_of(case, &read, "lastUpdatedAt"),
                updated,
                "case {case:?}: the auth type is not the policy: lastUpdatedAt must not move"
            );
        }
    }
}

/// Rule 5: a `policy` the service cannot read as a JSON policy document is refused in
/// AWS's words, and the document the resource already carried is left exactly as it was.
///
/// The rows span the ways a value fails to be a policy document: it is not JSON at all,
/// it is JSON that does not parse, or it parses as something other than an object.
#[tokio::test]
async fn an_unreadable_auth_policy_is_refused_and_leaves_the_stored_one_alone() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(
            serde_json::json!({ "name": "malformed-auth", "authType": AUTH_TYPE_AWS_IAM }),
        )
        .await;
    assert_eq!(
        network.status, 201,
        "fixture service network: expected 201, got {} / {}",
        network.status, network.body
    );
    let id = network.str_field("id");

    let rows: Vec<(&str, String)> = vec![
        ("an empty document", String::new()),
        ("whitespace only", "   ".to_string()),
        ("text that is not JSON", "not a policy".to_string()),
        (
            "an object that is never closed",
            r#"{"Version":"2012-10-17""#.to_string(),
        ),
        (
            "single-quoted names, which JSON does not admit",
            "{'Version':'2012-10-17'}".to_string(),
        ),
        (
            "a trailing comma, which JSON does not admit",
            r#"{"Version":"2012-10-17",}"#.to_string(),
        ),
        ("a JSON array, not a policy document", "[]".to_string()),
        (
            "a JSON string, not a policy document",
            r#""2012-10-17""#.to_string(),
        ),
        ("a JSON number, not a policy document", "42".to_string()),
        ("JSON null, not a policy document", "null".to_string()),
        ("two documents in one value", "{} {}".to_string()),
    ];

    // Before any policy is attached, a refused put must not leave a half-made one.
    for (case, policy) in &rows {
        assert_malformed_auth_policy(case, &put_auth_policy(&lattice, &id, policy).await);
        assert_no_auth_policy(case, &get_auth_policy(&lattice, &id).await);
    }

    // With a policy attached, a refused put must leave that document and its state
    // untouched.
    let stored = r#"{"Version":"2012-10-17","Id":"kept"}"#;
    assert_eq!(
        put_auth_policy(&lattice, &id, stored).await.status,
        200,
        "the fixture put expected 200"
    );
    for (case, policy) in &rows {
        assert_malformed_auth_policy(case, &put_auth_policy(&lattice, &id, policy).await);
        assert_auth_policy(
            case,
            &get_auth_policy(&lattice, &id).await,
            stored,
            STATE_ACTIVE,
        );
    }
}

/// Rule 5's not-founds: an identifier of the modelled shape that names no persisted
/// resource is a 404 on every auth-policy operation, with AWS's not-found construction
/// over the kind the identifier named.
///
/// The rows span the three kinds `ResourceIdentifier` enumerates — including the resource
/// configuration, which no implemented create can mint — in BOTH forms that member
/// admits, over all three operations.
#[tokio::test]
async fn an_auth_policy_call_naming_no_persisted_resource_is_the_modelled_not_found() {
    let lattice = Lattice::new();

    // (case, the identifier the request names, the CamelCase kind token AWS's message
    // names, the resourceType token). Both tokens are written out here rather than read
    // from the service, so no row can pass by agreeing with the code under test.
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a service network id that was never created",
            absent_id(PREFIX_SERVICE_NETWORK, "1"),
            "ServiceNetwork",
            "SERVICE_NETWORK",
        ),
        (
            "the ARN of a service network that was never created",
            expected_arn(
                ARN_KIND_SERVICE_NETWORK,
                &absent_id(PREFIX_SERVICE_NETWORK, "2"),
            ),
            "ServiceNetwork",
            "SERVICE_NETWORK",
        ),
        (
            "a service id that was never created",
            absent_id(PREFIX_SERVICE, "y"),
            "Service",
            "SERVICE",
        ),
        (
            "the ARN of a service that was never created",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE, "z")),
            "Service",
            "SERVICE",
        ),
        (
            "a resource configuration id, a kind nothing can create yet",
            absent_id(PREFIX_RESOURCE_CONFIGURATION, "3f4a"),
            "ResourceConfiguration",
            "RESOURCE_CONFIGURATION",
        ),
        (
            "the ARN of a resource configuration, the same kind by its ARN",
            expected_arn(
                ARN_KIND_RESOURCE_CONFIGURATION,
                &absent_id(PREFIX_RESOURCE_CONFIGURATION, "b7"),
            ),
            "ResourceConfiguration",
            "RESOURCE_CONFIGURATION",
        ),
    ];

    for (row, identifier, label, resource_type) in rows {
        for (operation, answer) in [
            (
                "PutAuthPolicy",
                put_auth_policy(&lattice, &identifier, r#"{"Version":"2012-10-17"}"#).await,
            ),
            ("GetAuthPolicy", get_auth_policy(&lattice, &identifier).await),
            (
                "DeleteAuthPolicy",
                delete_auth_policy(&lattice, &identifier).await,
            ),
        ] {
            let case = format!("{operation}: {row}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
                answer.error_code,
                answer.body
            );
            assert_eq!(
                message_of(&case, &answer),
                format!("{label} {identifier} cannot be found."),
                "case {case:?}: expected AWS's not-found construction"
            );
            assert_eq!(
                answer.body["resourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected the kind's resourceType token, got {}",
                answer.body
            );
            assert_eq!(
                answer.body["resourceId"].as_str(),
                Some(identifier.as_str()),
                "case {case:?}: resourceId must name what the request named, got {}",
                answer.body
            );
        }
    }
}

/// Rule 5's modelled-constraint refusals: a `policy` past `AuthPolicyString`'s `@length`
/// and a `resourceIdentifier` outside its `@length` or `@pattern` are 400s, and none of
/// them changes the policy the named resource carries.
#[tokio::test]
async fn an_auth_policy_call_outside_a_modelled_constraint_is_400_and_writes_nothing() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(
            serde_json::json!({ "name": "auth-constraints", "authType": AUTH_TYPE_AWS_IAM }),
        )
        .await;
    assert_eq!(
        network.status, 201,
        "fixture service network: expected 201, got {} / {}",
        network.status, network.body
    );
    let id = network.str_field("id");

    let stored = r#"{"Version":"2012-10-17","Id":"kept"}"#;
    assert_eq!(
        put_auth_policy(&lattice, &id, stored).await.status,
        200,
        "the fixture put expected 200"
    );

    // (case, the identifier the request names, the policy it submits)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a policy one character past the modelled ceiling",
            id.clone(),
            {
                let head = r#"{"Version":"2012-10-17","Id":""#;
                let tail = r#"","Statement":[]}"#;
                let filler = AUTH_POLICY_MAX_LENGTH - head.len() - tail.len() + 1;
                format!("{head}{}{tail}", "b".repeat(filler))
            },
        ),
        (
            "an identifier shorter than the modelled minimum",
            "sn-0".to_string(),
            stored.to_string(),
        ),
        (
            "an identifier whose suffix is outside the modelled alphabet",
            format!("{PREFIX_SERVICE_NETWORK}{}", "A".repeat(17)),
            stored.to_string(),
        ),
        (
            "an ARN whose kind segment and id prefix disagree",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE_NETWORK, "0")),
            stored.to_string(),
        ),
    ];

    for (case, identifier, policy) in rows {
        let answer = put_auth_policy(&lattice, &identifier, &policy).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected the ValidationException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        assert_auth_policy(
            case,
            &get_auth_policy(&lattice, &id).await,
            stored,
            STATE_ACTIVE,
        );
    }
}

// --- the TAGS slice (rules 6, 7 and 8) -------------------------------------------

/// The ARN resource segments and id prefixes of the kinds a tag `resourceArn` can name
/// beyond the three a policy ARN does. SOURCE: each kind's own modelled ARN pattern
/// (`smithy_query(query="shape",shape="ServiceNetworkServiceAssociationArn,ServiceNetworkVpcAssociationArn,ListenerArn,RuleArn,TargetGroupArn")`),
/// which pairs the segment with the id prefix.
const ARN_KIND_SERVICE_ASSOCIATION: &str = "servicenetworkserviceassociation";
const ARN_KIND_VPC_ASSOCIATION: &str = "servicenetworkvpcassociation";
const ARN_KIND_LISTENER: &str = "listener";
const ARN_KIND_RULE: &str = "rule";
const ARN_KIND_TARGET_GROUP: &str = "targetgroup";
const PREFIX_SERVICE_ASSOCIATION: &str = "snsa-";
const PREFIX_VPC_ASSOCIATION: &str = "snva-";
const PREFIX_LISTENER: &str = "listener-";
const PREFIX_RULE: &str = "rule-";
const PREFIX_TARGET_GROUP: &str = "tg-";

/// The `@length` ceiling the model puts on `TagMap`, which bounds both one request's
/// `tags` and the map `ListTagsForResource` reports
/// (`smithy_query(query="shape",shape="TagMap")`).
const TAG_MAP_MAX: usize = 200;

/// The `@length` ceilings of one tag's key and value
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`: `TagKey` 1..128, `TagValue`
/// 0..256).
const TAG_KEY_MAX: usize = 128;
const TAG_VALUE_MAX: usize = 256;

/// `POST /tags/{resourceArn}` with the pairs this case writes.
async fn tag_resource(lattice: &Lattice, resource_arn: &str, pairs: &[(String, String)]) -> Answer {
    let mut tags = serde_json::Map::new();
    for (key, value) in pairs {
        tags.insert(key.clone(), serde_json::json!(value));
    }
    lattice
        .call(
            "POST",
            &format!("/tags/{}", as_label(resource_arn)),
            Some(serde_json::json!({ "tags": tags })),
        )
        .await
}

/// `GET /tags/{resourceArn}`.
async fn list_tags(lattice: &Lattice, resource_arn: &str) -> Answer {
    lattice
        .call("GET", &format!("/tags/{}", as_label(resource_arn)), None)
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…`, with one query entry per key: `tagKeys` is
/// bound to the query string
/// (`smithy_query(query="operation",operation="UntagResource")`). Every key a case removes
/// here is made of characters that need no escaping.
async fn untag_resource(lattice: &Lattice, resource_arn: &str, keys: &[&str]) -> Answer {
    let query: Vec<String> = keys.iter().map(|key| format!("tagKeys={key}")).collect();
    lattice
        .call(
            "DELETE",
            &format!("/tags/{}?{}", as_label(resource_arn), query.join("&")),
            None,
        )
        .await
}

/// The tag map a `ListTagsForResource` answer reports, as a sorted map so two answers
/// compare by content rather than by order.
fn listed_tags(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} / {}",
        answer.status, answer.body
    );
    let reported = answer.body["tags"].as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: ListTagsForResource must report the modelled tags map, got {}",
            answer.body
        )
    });
    reported
        .iter()
        .map(|(key, value)| {
            let text = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: the tag {key:?} must be a string, got {value}")
            });
            (key.clone(), text.to_string())
        })
        .collect()
}

/// One resource a tag call can name: how a case labels it, its ARN, and the tags its
/// create stored.
struct Taggable {
    label: &'static str,
    arn: String,
    created_with: BTreeMap<String, String>,
}

/// The ARN of a resource a create must have minted, with the status that create answers.
fn created_arn(case: &str, expected_status: u16, answer: &Answer) -> String {
    assert_eq!(
        answer.status, expected_status,
        "case fixture {case}: expected {expected_status}, got {} / {}",
        answer.status, answer.body
    );
    answer.str_field("arn")
}

/// The map form of the `tags` a create sent, so a tag read can be compared against it.
fn sent_tags(tags: &serde_json::Value) -> BTreeMap<String, String> {
    tags.as_object()
        .map(|members| {
            members
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// One resource of every kind this emulator mints, so the tag rules are asserted over the
/// KIND of resource as well as over the tags written — including a listener and a rule,
/// whose ARNs nest under another resource's, and the two associations, whose creates answer
/// 200 rather than 201.
///
/// Some creates send `tags` and some send none, so the table also covers the map a create
/// stored: the tag operations read and write that same map.
async fn every_taggable_kind(lattice: &Lattice) -> Vec<Taggable> {
    let network_tags = serde_json::json!({ "made-by": "create-service-network" });
    let network = lattice
        .create_service_network(serde_json::json!({
            "name": "tagged-network",
            "tags": network_tags,
        }))
        .await;
    let network_arn = created_arn("service network", 201, &network);
    let network_id = network.str_field("id");

    let service = lattice
        .create_service(serde_json::json!({ "name": "tagged-service" }))
        .await;
    let service_arn = created_arn("service", 201, &service);
    let service_id = service.str_field("id");

    let association_tags = serde_json::json!({ "made-by": "create-association" });
    let association = lattice
        .call(
            "POST",
            "/servicenetworkserviceassociations",
            Some(serde_json::json!({
                "serviceIdentifier": service_id,
                "serviceNetworkIdentifier": network_id,
                "tags": association_tags,
            })),
        )
        .await;
    let association_arn = created_arn("service association", 200, &association);

    let vpc_association = lattice
        .call(
            "POST",
            "/servicenetworkvpcassociations",
            Some(serde_json::json!({
                "serviceNetworkIdentifier": network_id,
                "vpcIdentifier": crate::target_groups::A_VPC,
            })),
        )
        .await;
    let vpc_association_arn = created_arn("VPC association", 200, &vpc_association);

    let listener = lattice
        .call(
            "POST",
            &format!("/services/{service_id}/listeners"),
            Some(serde_json::json!({
                "name": "tagged-listener",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            })),
        )
        .await;
    let listener_arn = created_arn("listener", 201, &listener);
    let listener_id = listener.str_field("id");

    let rule_tags = serde_json::json!({ "made-by": "create-rule" });
    let rule = lattice
        .call(
            "POST",
            &format!("/services/{service_id}/listeners/{listener_id}/rules"),
            Some(serde_json::json!({
                "name": "tagged-rule",
                "priority": 7,
                "match": { "httpMatch": { "pathMatch": { "match": { "prefix": "/" } } } },
                "action": { "fixedResponse": { "statusCode": 404 } },
                "tags": rule_tags,
            })),
        )
        .await;
    let rule_arn = created_arn("rule", 201, &rule);

    let target_group = lattice
        .call(
            "POST",
            "/targetgroups",
            Some(serde_json::json!({
                "name": "tagged-target-group",
                "type": "IP",
                "config": { "protocol": "HTTP", "vpcIdentifier": crate::target_groups::A_VPC },
            })),
        )
        .await;
    let target_group_arn = created_arn("target group", 201, &target_group);

    vec![
        Taggable {
            label: "a service network",
            arn: network_arn,
            created_with: sent_tags(&network_tags),
        },
        Taggable {
            label: "a service",
            arn: service_arn,
            created_with: BTreeMap::new(),
        },
        Taggable {
            label: "a service-network/service association",
            arn: association_arn,
            created_with: sent_tags(&association_tags),
        },
        Taggable {
            label: "a service-network/VPC association",
            arn: vpc_association_arn,
            created_with: BTreeMap::new(),
        },
        Taggable {
            label: "a listener, whose ARN nests under its service's",
            arn: listener_arn,
            created_with: BTreeMap::new(),
        },
        Taggable {
            label: "a rule, whose ARN nests under its listener's",
            arn: rule_arn,
            created_with: sent_tags(&rule_tags),
        },
        Taggable {
            label: "a target group",
            arn: target_group_arn,
            created_with: BTreeMap::new(),
        },
    ]
}

/// The tag maps the write table spans. No single map is the requirement: the rule is that
/// whatever pairs a call sends are merged into the resource's map, so the rows differ in
/// pair count, in whether they replace a key the resource already carries, and in the
/// characters and lengths their keys and values use — up to the `@length` ceilings
/// `TagKey` and `TagValue` declare.
fn tag_write_rows() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "one pair",
            vec![("team".to_string(), "payments".to_string())],
        ),
        (
            "a second key, which leaves the first alone",
            vec![("env".to_string(), "prod".to_string())],
        ),
        (
            "two pairs, one replacing a held key beside its case-different sibling",
            vec![
                ("team".to_string(), "platform".to_string()),
                ("TEAM".to_string(), "shouting".to_string()),
            ],
        ),
        (
            "an empty value, which TagValue's @length(0, 256) admits",
            vec![("note".to_string(), String::new())],
        ),
        (
            "a non-ASCII key and value",
            vec![("équipe".to_string(), "paiements ❤".to_string())],
        ),
        (
            "a key and a value at their modelled @length ceilings",
            vec![("k".repeat(TAG_KEY_MAX), "v".repeat(TAG_VALUE_MAX))],
        ),
    ]
}

/// Rule 6, over the two tables: every kind of resource, and every row of tags written onto
/// each. The recorded case — tags written on two resources and read back on each — is one
/// row of it.
#[tokio::test]
async fn tags_are_merged_into_the_map_of_the_resource_the_arn_names() {
    let lattice = Lattice::new();
    let resources = every_taggable_kind(&lattice).await;

    // What each resource must report once every row has been written onto it.
    let mut expected: Vec<BTreeMap<String, String>> = Vec::new();
    for resource in &resources {
        let label = resource.label;
        // The tags a create stored ARE the resource's tag map: a tag read reports them
        // without anything else having written them.
        let case = format!("{label} as created");
        let mut held = listed_tags(&case, &list_tags(&lattice, &resource.arn).await);
        assert_eq!(
            held, resource.created_with,
            "case {case:?}: expected the tags the create stored {:?}, got {held:?}",
            resource.created_with
        );

        for (row, pairs) in tag_write_rows() {
            let case = format!("{label} / {row}");
            let written = tag_resource(&lattice, &resource.arn, &pairs).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected 200 from TagResource, got {} / {}",
                written.status, written.body
            );
            for (key, value) in &pairs {
                held.insert(key.clone(), value.clone());
            }
            let listed = listed_tags(&case, &list_tags(&lattice, &resource.arn).await);
            assert_eq!(
                listed, held,
                "case {case:?}: expected the merged map {held:?}, got {listed:?}"
            );
        }
        expected.push(held);
    }

    // Each resource carries its OWN map: having written the same keys onto every kind, a
    // read of each still reports exactly what was written onto that one.
    for (resource, held) in resources.iter().zip(&expected) {
        let case = format!("{} after every other resource was tagged", resource.label);
        let listed = listed_tags(&case, &list_tags(&lattice, &resource.arn).await);
        assert_eq!(
            listed, *held,
            "case {case:?}: expected this resource's own map {held:?}, got {listed:?}"
        );
    }
}

/// Rule 7, over a table of key sets and two kinds of resource.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "untag-network" }))
        .await;
    let network_arn = created_arn("service network", 201, &network);
    let target_group = lattice
        .call(
            "POST",
            "/targetgroups",
            Some(serde_json::json!({
                "name": "untag-target-group",
                "type": "IP",
                "config": { "protocol": "HTTP", "vpcIdentifier": crate::target_groups::A_VPC },
            })),
        )
        .await;
    let target_group_arn = created_arn("target group", 201, &target_group);

    // (case, the keys the call removes, the keys the resource must still carry)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one key", vec!["a"], vec!["b", "c", "keep"]),
        ("two keys at once", vec!["b", "c"], vec!["keep"]),
        (
            "a key the resource never carried",
            vec!["never-written"],
            vec!["keep"],
        ),
        ("a key already removed", vec!["a"], vec!["keep"]),
        ("the last key it carries", vec!["keep"], vec![]),
    ];

    for (label, arn) in [
        ("a service network", &network_arn),
        ("a target group", &target_group_arn),
    ] {
        let initial: Vec<(String, String)> = ["a", "b", "c", "keep"]
            .iter()
            .map(|key| ((*key).to_string(), format!("value-of-{key}")))
            .collect();
        assert_eq!(
            tag_resource(&lattice, arn, &initial).await.status,
            200,
            "case fixture tags on {label}: expected 200 from TagResource"
        );

        for (row, removed, remaining) in &rows {
            let case = format!("{label} / {row}");
            let answer = untag_resource(&lattice, arn, removed).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from UntagResource, got {} / {}",
                answer.status, answer.body
            );
            let listed = listed_tags(&case, &list_tags(&lattice, arn).await);
            let expected: BTreeMap<String, String> = remaining
                .iter()
                .map(|key| ((*key).to_string(), format!("value-of-{key}")))
                .collect();
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the remaining tags {expected:?}, got {listed:?}"
            );
        }
    }
}

/// The ARNs rule 8 spans: one of every kind a tag `resourceArn` can name, each of the
/// modelled shape and naming a resource nothing ever created, plus two an ARN can be
/// without naming a resource of this service at all.
fn absent_resource_arns() -> Vec<(&'static str, String)> {
    let absent_service = absent_id(PREFIX_SERVICE, "1");
    let absent_listener = absent_id(PREFIX_LISTENER, "2");
    vec![
        (
            "a service network",
            expected_arn(
                ARN_KIND_SERVICE_NETWORK,
                &absent_id(PREFIX_SERVICE_NETWORK, "0"),
            ),
        ),
        (
            "a service",
            expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE, "0")),
        ),
        (
            "a service-network/service association",
            expected_arn(
                ARN_KIND_SERVICE_ASSOCIATION,
                &absent_id(PREFIX_SERVICE_ASSOCIATION, "0"),
            ),
        ),
        (
            "a service-network/VPC association",
            expected_arn(
                ARN_KIND_VPC_ASSOCIATION,
                &absent_id(PREFIX_VPC_ASSOCIATION, "0"),
            ),
        ),
        (
            "a listener under a service nothing created",
            expected_arn(
                &format!("{ARN_KIND_SERVICE}/{absent_service}/{ARN_KIND_LISTENER}"),
                &absent_listener,
            ),
        ),
        (
            "a rule under a listener nothing created",
            expected_arn(
                &format!(
                    "{ARN_KIND_SERVICE}/{absent_service}/{ARN_KIND_LISTENER}/{absent_listener}/{ARN_KIND_RULE}"
                ),
                &absent_id(PREFIX_RULE, "3"),
            ),
        ),
        (
            "a target group",
            expected_arn(ARN_KIND_TARGET_GROUP, &absent_id(PREFIX_TARGET_GROUP, "0")),
        ),
        (
            "a resource configuration, a kind no create of this emulator mints yet",
            expected_arn(
                ARN_KIND_RESOURCE_CONFIGURATION,
                &absent_id(PREFIX_RESOURCE_CONFIGURATION, "0"),
            ),
        ),
        (
            "an ARN whose resource part names no kind of this service",
            expected_arn("mystery", "thing-0000000000000"),
        ),
        (
            "an ARN of a known kind with trailing text after the id",
            format!(
                "{}-does-not-exist",
                expected_arn(
                    ARN_KIND_SERVICE_NETWORK,
                    &absent_id(PREFIX_SERVICE_NETWORK, "4")
                )
            ),
        ),
    ]
}

/// The refusal every row of rule 8's not-found table must get: HTTP 404, the
/// `NotFoundException` wire code, and the fixed sentence, which names nothing the request
/// carried.
const TAG_NOT_FOUND_MESSAGE: &str = "Resource was not found";

fn assert_tag_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: expected the NotFoundException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    let message = message_of(case, answer);
    assert_eq!(
        message, TAG_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the tag front-end's refusal {TAG_NOT_FOUND_MESSAGE:?}, \
         got {message:?}"
    );
}

/// Rule 8's not-found half, over every kind of ARN and all three tag operations.
#[tokio::test]
async fn a_tag_call_naming_no_resource_is_refused_as_the_tag_front_end_refuses_one() {
    let lattice = Lattice::new();
    // A persisted resource whose tags must be untouched by any of the refused calls.
    let network = lattice
        .create_service_network(serde_json::json!({
            "name": "tag-not-found-bystander",
            "tags": { "kept": "yes" },
        }))
        .await;
    let bystander = created_arn("bystander service network", 201, &network);

    let written = vec![("team".to_string(), "payments".to_string())];
    for (row, arn) in absent_resource_arns() {
        for (operation, answer) in [
            ("TagResource", tag_resource(&lattice, &arn, &written).await),
            ("ListTagsForResource", list_tags(&lattice, &arn).await),
            (
                "UntagResource",
                untag_resource(&lattice, &arn, &["team"]).await,
            ),
        ] {
            let case = format!("{operation} on {row}");
            assert_tag_not_found(&case, &answer);
        }
    }

    let listed = listed_tags("the bystander", &list_tags(&lattice, &bystander).await);
    let expected: BTreeMap<String, String> = [("kept".to_string(), "yes".to_string())]
        .into_iter()
        .collect();
    assert_eq!(
        listed, expected,
        "a refused tag call must leave another resource's tags alone: expected \
         {expected:?}, got {listed:?}"
    );
}

/// Rule 8's ceiling half: the modelled `TagMap` bound is what a resource can hold, so a
/// write that would add a key past it is refused and changes nothing, while one that only
/// replaces a held key's value is accepted at the ceiling.
#[tokio::test]
async fn a_tag_write_past_the_modelled_ceiling_is_refused_and_writes_nothing() {
    let lattice = Lattice::new();
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "tag-ceiling" }))
        .await;
    let arn = created_arn("service network", 201, &network);

    let full: Vec<(String, String)> = (0..TAG_MAP_MAX)
        .map(|index| (format!("key-{index}"), index.to_string()))
        .collect();
    assert_eq!(
        tag_resource(&lattice, &arn, &full).await.status,
        200,
        "case the whole ceiling at once: expected 200 from TagResource"
    );
    let at_ceiling = listed_tags(
        "the whole ceiling at once",
        &list_tags(&lattice, &arn).await,
    );
    assert_eq!(
        at_ceiling.len(),
        TAG_MAP_MAX,
        "case the whole ceiling at once: expected {TAG_MAP_MAX} tags, got {}",
        at_ceiling.len()
    );

    let refused = tag_resource(
        &lattice,
        &arn,
        &[("one-too-many".to_string(), "x".to_string())],
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case one key past the ceiling: expected 400, got {} / {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ValidationException"),
        "case one key past the ceiling: expected the ValidationException wire code, got {:?} / {}",
        refused.error_code,
        refused.body
    );
    let unchanged = listed_tags("one key past the ceiling", &list_tags(&lattice, &arn).await);
    assert_eq!(
        unchanged, at_ceiling,
        "case one key past the ceiling: the refused write must store nothing, expected \
         {at_ceiling:?}, got {unchanged:?}"
    );

    // Replacing a held key adds no entry, so it is still inside the ceiling.
    let replaced = tag_resource(
        &lattice,
        &arn,
        &[("key-0".to_string(), "replaced".to_string())],
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "case replacing a held key at the ceiling: expected 200, got {} / {}",
        replaced.status, replaced.body
    );
    let after = listed_tags(
        "replacing a held key at the ceiling",
        &list_tags(&lattice, &arn).await,
    );
    assert_eq!(
        after.len(),
        TAG_MAP_MAX,
        "case replacing a held key at the ceiling: expected {TAG_MAP_MAX} tags, got {}",
        after.len()
    );
    assert_eq!(
        after.get("key-0").map(String::as_str),
        Some("replaced"),
        "case replacing a held key at the ceiling: expected the new value, got {:?}",
        after.get("key-0")
    );
}
