// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the form of the `OperationId` every asynchronous Cloud Map
//! call answers with: a two-segment opaque token, the same form whichever operation
//! filed the record, resolvable through `GetOperation`, and never derived from the
//! request.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`), and the
//! harness is the one `cloud_map_core` already builds.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{emulator, failure, namespace, ok, service};

/// How many characters the leading segment of an `OperationId` carries.
///
/// The model constrains only the overall length (`OperationId`, `length 0..255`), so the
/// form comes from the operation ids AWS answers these calls with: a two-segment token
/// such as the `RegisterInstance`/`GetOperation` example id
/// `4yejorelbukcjzpnr6tlmrghsjwpngf4-k95yg2u7` — 32 lowercase alphanumerics, a hyphen and
/// 8 more.
const BODY_DIGITS: usize = 32;

/// How many characters the segment behind the hyphen carries. See [`BODY_DIGITS`].
const SUFFIX_DIGITS: usize = 8;

/// The published maximum of `OperationId`
/// (`smithy_query(query="shape", shape="OperationId")`: `length {min: 0, max: 255}`).
const OPERATION_ID_MAX: usize = 255;

/// A VPC id of the shape `CreatePrivateDnsNamespace$Vpc` takes (`ResourceId`, 0..64).
const VPC: &str = "vpc-0123456789abcdef0";

/// Dispatch an asynchronous operation that must succeed and return its `OperationId`.
async fn operation_id(router: &MultiServiceRouter, operation: &str, body: Value) -> String {
    let answered = ok(router, operation, body).await;
    answered["OperationId"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation} must answer an OperationId, got {answered}"))
        .to_string()
}

/// The resource of kind `key` the operation `id` targets, as `GetOperation` reports it.
async fn target(router: &MultiServiceRouter, id: &str, key: &str) -> String {
    let filed = ok(router, "GetOperation", json!({ "OperationId": id })).await;
    filed["Operation"]["Targets"][key]
        .as_str()
        .unwrap_or_else(|| panic!("the operation {id} must target a {key}, got {filed}"))
        .to_string()
}

/// Assert `id` has the published operation-id form, naming `case` and the actual value.
///
/// The form is checked member by member rather than against one recorded literal: two
/// lowercase-alphanumeric runs of [`BODY_DIGITS`] and [`SUFFIX_DIGITS`] characters
/// joined by a single hyphen, inside the modelled length.
fn assert_operation_id_form(case: &str, id: &str) {
    assert!(
        id.len() <= OPERATION_ID_MAX,
        "case {case:?}: an OperationId is at most {OPERATION_ID_MAX} characters, got {id:?}"
    );
    let mut segments = id.split('-');
    let body = segments.next().unwrap_or_default();
    let suffix = segments.next().unwrap_or_default();
    assert!(
        segments.next().is_none(),
        "case {case:?}: expected one hyphen in the OperationId, got {id:?}"
    );
    let alphanumeric =
        |segment: &str| segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    assert!(
        body.len() == BODY_DIGITS && alphanumeric(body),
        "case {case:?}: expected {BODY_DIGITS} lowercase alphanumerics before the hyphen, \
         got {body:?} in {id:?}"
    );
    assert!(
        suffix.len() == SUFFIX_DIGITS && alphanumeric(suffix),
        "case {case:?}: expected {SUFFIX_DIGITS} lowercase alphanumerics after the hyphen, \
         got {suffix:?} in {id:?}"
    );
}

/// The rule: every asynchronous Cloud Map call answers an `OperationId` of ONE form —
/// an opaque two-segment token — whichever operation filed the record, and the token
/// resolves through `GetOperation`.
///
/// The rows span the class: a create, an update and a delete of a namespace, each of the
/// three namespace kinds, a service update, and an instance register and deregister, so
/// the operation kind, the resource kind and the target kind all differ between rows.
#[tokio::test]
async fn every_asynchronous_call_answers_an_operation_id_of_the_published_form() {
    let router = emulator();

    let mut rows: Vec<(&str, String)> = Vec::new();

    // A create of each namespace kind. `namespace` (the shared harness) creates the
    // HTTP one and hands back the operation its create filed.
    let (create_http, http_namespace) = namespace(&router, "opid-http-ns").await;
    rows.push(("CreateHttpNamespace", create_http));

    let create_private = operation_id(
        &router,
        "CreatePrivateDnsNamespace",
        json!({ "Name": "opid-private.example.com", "Vpc": VPC }),
    )
    .await;
    let private_namespace = target(&router, &create_private, "NAMESPACE").await;
    rows.push(("CreatePrivateDnsNamespace", create_private));

    let create_public = operation_id(
        &router,
        "CreatePublicDnsNamespace",
        json!({ "Name": "opid-public.example.com" }),
    )
    .await;
    let public_namespace = target(&router, &create_public, "NAMESPACE").await;
    rows.push(("CreatePublicDnsNamespace", create_public));

    // An update of each namespace kind: the change shapes differ, the id form does not.
    rows.push((
        "UpdateHttpNamespace",
        operation_id(
            &router,
            "UpdateHttpNamespace",
            json!({ "Id": http_namespace, "Namespace": { "Description": "an http namespace" } }),
        )
        .await,
    ));
    rows.push((
        "UpdatePrivateDnsNamespace",
        operation_id(
            &router,
            "UpdatePrivateDnsNamespace",
            json!({ "Id": private_namespace,
                    "Namespace": { "Properties": { "DnsProperties": { "SOA": { "TTL": 60 } } } } }),
        )
        .await,
    ));
    rows.push((
        "UpdatePublicDnsNamespace",
        operation_id(
            &router,
            "UpdatePublicDnsNamespace",
            json!({ "Id": public_namespace, "Namespace": { "Description": "a public namespace" } }),
        )
        .await,
    ));

    // A service update — `UpdateService` applies to a service whose settings include
    // Route 53 records, so this one lives in the public DNS namespace — and an instance
    // registered and deregistered under a service of the HTTP namespace.
    let dns_service = ok(
        &router,
        "CreateService",
        json!({ "Name": "opid-dns-svc", "NamespaceId": public_namespace,
                "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 60 }] } }),
    )
    .await["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string();
    rows.push((
        "UpdateService",
        operation_id(
            &router,
            "UpdateService",
            json!({ "Id": dns_service, "Service": { "Description": "a service",
                    "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 30 }] } } }),
        )
        .await,
    ));
    let service_id = service(&router, &http_namespace, "opid-svc").await;
    rows.push((
        "RegisterInstance",
        operation_id(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-1",
                    "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
        )
        .await,
    ));
    rows.push((
        "DeregisterInstance",
        operation_id(
            &router,
            "DeregisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-1" }),
        )
        .await,
    ));

    // A delete, of a namespace nothing else in this test holds a service in.
    let (_, doomed) = namespace(&router, "opid-doomed-ns").await;
    rows.push((
        "DeleteNamespace",
        operation_id(&router, "DeleteNamespace", json!({ "Id": doomed })).await,
    ));

    let mut seen = BTreeSet::new();
    for (case, id) in &rows {
        assert_operation_id_form(case, id);
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: every operation gets its own id, but {id:?} was already answered"
        );

        // The id is what every later poll is keyed on, so it must name the record that
        // was filed — the form is only half the contract.
        let filed = ok(&router, "GetOperation", json!({ "OperationId": id })).await;
        let reported = filed["Operation"]["Id"].as_str().unwrap_or_default();
        assert_eq!(
            reported, id,
            "case {case:?}: GetOperation must report the id it was asked about, got {filed}"
        );
    }
}

/// Which form of the `Arn`-modelled `DeleteNamespaceRequest$Id` names the namespace a
/// delete removes (`smithy_query(query="operation", operation="DeleteNamespace")`: `Id`
/// targets `Arn`, "the ID or Amazon Resource Name (ARN) of the namespace").
#[derive(Debug, Clone, Copy)]
enum Reference {
    Id,
    Arn,
}

/// The same rule read through the delete, the call the id is most often split by: the
/// `OperationId` `DeleteNamespace` answers carries the two-segment form whichever kind of
/// namespace it removed and whichever form named it, and the WHOLE token is the handle —
/// neither segment on its own resolves.
///
/// The rows span the namespace kind and the reference form, the two things this call
/// varies along; the form itself belongs to the store's operation-id generator, so it
/// cannot depend on either.
#[tokio::test]
async fn a_delete_answers_an_operation_id_of_the_published_form_for_every_namespace_kind() {
    let router = emulator();

    let rows: Vec<(&str, &str, Value, Reference)> = vec![
        (
            "an http namespace named by id",
            "CreateHttpNamespace",
            json!({ "Name": "opid-del-http-id" }),
            Reference::Id,
        ),
        (
            "an http namespace named by ARN",
            "CreateHttpNamespace",
            json!({ "Name": "opid-del-http-arn" }),
            Reference::Arn,
        ),
        (
            "a private dns namespace named by id",
            "CreatePrivateDnsNamespace",
            json!({ "Name": "opid-del-private-id.example.com", "Vpc": VPC }),
            Reference::Id,
        ),
        (
            "a private dns namespace named by ARN",
            "CreatePrivateDnsNamespace",
            json!({ "Name": "opid-del-private-arn.example.com", "Vpc": VPC }),
            Reference::Arn,
        ),
        (
            "a public dns namespace named by id",
            "CreatePublicDnsNamespace",
            json!({ "Name": "opid-del-public-id.example.com" }),
            Reference::Id,
        ),
        (
            "a public dns namespace named by ARN",
            "CreatePublicDnsNamespace",
            json!({ "Name": "opid-del-public-arn.example.com" }),
            Reference::Arn,
        ),
    ];

    let mut seen = BTreeSet::new();
    for (case, create, body, reference) in rows {
        let name = body["Name"]
            .as_str()
            .expect("every row names its namespace")
            .to_string();
        let created = operation_id(&router, create, body).await;
        let namespace_id = target(&router, &created, "NAMESPACE").await;
        let arn = ok(&router, "GetNamespace", json!({ "Id": namespace_id.clone() })).await
            ["Namespace"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a stored namespace carries its ARN"))
            .to_string();

        let named = match reference {
            Reference::Id => namespace_id.clone(),
            Reference::Arn => arn.clone(),
        };
        let deleted = operation_id(&router, "DeleteNamespace", json!({ "Id": named })).await;

        assert_operation_id_form(case, &deleted);
        assert!(
            seen.insert(deleted.clone()),
            "case {case:?}: every delete files its own id, but {deleted:?} was already answered"
        );

        // The token is what a later poll is keyed on: it must name the record the delete
        // filed, for the namespace it removed.
        let filed = ok(
            &router,
            "GetOperation",
            json!({ "OperationId": deleted.clone() }),
        )
        .await;
        assert_eq!(
            filed["Operation"]["Id"],
            json!(deleted),
            "case {case:?}: GetOperation must report the id it was asked about, got {filed}"
        );
        assert_eq!(
            filed["Operation"]["Type"],
            json!("DELETE_NAMESPACE"),
            "case {case:?}: the filed operation is the delete, got {filed}"
        );
        assert_eq!(
            filed["Operation"]["Targets"]["NAMESPACE"],
            json!(namespace_id),
            "case {case:?}: the operation targets the namespace that was deleted, got {filed}"
        );

        // The hyphen joins two segments of ONE handle rather than separating two usable
        // ones, so a client that splits the id and polls with a piece resolves nothing.
        let (body_segment, suffix_segment) = deleted
            .split_once('-')
            .unwrap_or_else(|| panic!("case {case:?}: the form was asserted above"));
        for partial in [body_segment, suffix_segment] {
            let (status, code) =
                failure(&router, "GetOperation", json!({ "OperationId": partial })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "OperationNotFound"),
                "case {case:?}: one segment {partial:?} of {deleted:?} is not an operation id"
            );
        }

        // Opaque: no part of the id is a value the request or the resource carried.
        for carried in [namespace_id.as_str(), name.as_str(), arn.as_str()] {
            assert!(
                !deleted.contains(carried),
                "case {case:?}: an OperationId carries nothing from the request, \
                 but {deleted:?} contains {carried:?}"
            );
        }
    }
}

/// The rule's other half: the token is OPAQUE — repeating a request verbatim files
/// another operation with another id, and no segment of an id is any value the request
/// carried.
#[tokio::test]
async fn an_operation_id_is_opaque_and_never_repeats() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "opid-opaque-ns").await;
    let service_id = service(&router, &namespace_id, "opid-opaque-svc").await;

    // The rows differ in how many times the identical request is repeated, which is the
    // dimension the uniqueness rule generalises over.
    let mut seen = BTreeSet::new();
    for repeat in 0..8 {
        let case = format!("register {repeat} of 8");
        let id = operation_id(
            &router,
            "RegisterInstance",
            json!({ "ServiceId": service_id, "InstanceId": "i-opaque",
                    "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" } }),
        )
        .await;
        assert_operation_id_form(&case, &id);
        assert!(
            seen.insert(id.clone()),
            "case {case:?}: an identical request must file its own operation, got {id:?} again"
        );
        for carried in [service_id.as_str(), "i-opaque", "10.0.0.1", &namespace_id] {
            assert!(
                !id.contains(carried),
                "case {case:?}: an OperationId carries nothing from the request, \
                 but {id:?} contains {carried:?}"
            );
        }
    }
}
