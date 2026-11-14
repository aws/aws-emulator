// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource and method DELETE paths: `DeleteMethod` and
//! `DeleteResource`, read back through `GetMethod`, `GetResource` and `GetResources`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource` helpers are this crate's existing harness, reused
//! from `rest_api_lifecycle`.
//!
//! The rule under test is one rule in two places: a delete removes the record it names,
//! and the records that record owns, from state — so every later read of any of them
//! answers the modelled `NotFoundException` (404), while a record the delete does not
//! own is untouched. The tables span what the rule generalises over: which HTTP verb is
//! deleted (the whole verb catalog, and a spelling of it the resource holds no method
//! under), and the shape of the deleted
//! subtree (a leaf, a chain, a branching tree, a mid node), not one recorded call.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method::HTTP_METHODS;

use crate::method_integration::put_method;
use crate::rest_api_lifecycle::{
    assert_failure, create_api, create_resource, page_items, string_member, Answer, Gateway,
};

/// `GetMethod` for `verb` on `resource`.
async fn get_method(gateway: &Gateway, api: &str, resource: &str, verb: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            None,
        )
        .await
}

/// `DeleteMethod` for `verb` on `resource`.
async fn delete_method(gateway: &Gateway, api: &str, resource: &str, verb: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            None,
        )
        .await
}

/// The verbs a resource's `resourceMethods` map reports, in sorted order.
async fn resource_verbs(gateway: &Gateway, api: &str, resource: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources/{resource}"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetResource, got {} with {:?}",
        answer.status, answer.body
    );
    let mut verbs: Vec<String> = answer.body["resourceMethods"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    verbs.sort();
    verbs
}

/// Create every path in `paths` under `api`, in the order given, and answer the id of
/// each — the root under `"/"`. A path's parent must appear earlier in the list.
async fn build_tree(
    gateway: &Gateway,
    api: &str,
    root: &str,
    paths: &[&str],
) -> BTreeMap<String, String> {
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    ids.insert("/".to_string(), root.to_string());
    for path in paths {
        let (parent_path, part) = path
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("case build_tree({path:?}): a path carries a separator"));
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let parent = ids.get(parent_path).unwrap_or_else(|| {
            panic!("case build_tree({path:?}): the parent {parent_path:?} must be created first, got {ids:?}")
        });
        let created = create_resource(gateway, api, parent, part).await;
        assert_eq!(
            created["path"].as_str(),
            Some(*path),
            "case build_tree({path:?}): expected that path, got {:?}",
            created["path"]
        );
        let id = string_member(&created, "id", path);
        ids.insert((*path).to_string(), id);
    }
    ids
}

/// The paths an API reports through `GetResources`, in sorted order.
async fn listed_paths(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call("GET", &format!("/restapis/{api}/resources?limit=500"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetResources, got {} with {:?}",
        answer.status, answer.body
    );
    let mut paths: Vec<String> = page_items(&answer.body, case)
        .iter()
        .map(|item| string_member(item, "path", case))
        .collect();
    paths.sort();
    paths
}

#[tokio::test]
async fn deleting_a_method_removes_that_verb_and_leaves_the_others() {
    // Source of the verb catalog: `put_method::HTTP_METHODS`, the verbs API Gateway
    // accepts for a method plus its `ANY` catch-all. Every verb is a row, deleted in
    // turn, and a lowercase spelling of it is a row of its own: a method is addressed by
    // the verb it was created with, so another spelling names no method to delete.
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-deletes").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "method-deletes/pets");

    for verb in HTTP_METHODS {
        put_method(&gateway, &api, &resource_id, verb).await;
    }

    let mut remaining: Vec<String> = HTTP_METHODS.iter().map(|verb| (*verb).to_string()).collect();
    for (index, verb) in HTTP_METHODS.iter().enumerate() {
        // Rows alternate whether the verb is first addressed in a spelling the resource
        // holds no method under, which is refused and deletes nothing.
        if index % 2 == 0 {
            let lowercase = verb.to_ascii_lowercase();
            assert_failure(
                &delete_method(&gateway, &api, &resource_id, &lowercase).await,
                404,
                "NotFoundException",
                &format!("DeleteMethod({lowercase})"),
            );
        }
        let addressed = (*verb).to_string();
        let case = format!("DeleteMethod({addressed})");

        let deleted = delete_method(&gateway, &api, &resource_id, &addressed).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected the modelled 204, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {case}: expected an empty body, got {:?}",
            deleted.body
        );

        // The method is gone for a reader, and so is every other spelling of its verb,
        // and deleting it
        // again finds nothing to delete.
        for probe in [addressed.clone(), verb.to_ascii_lowercase()] {
            assert_failure(
                &get_method(&gateway, &api, &resource_id, &probe).await,
                404,
                "NotFoundException",
                &format!("{case} / GetMethod({probe})"),
            );
        }
        assert_failure(
            &delete_method(&gateway, &api, &resource_id, &addressed).await,
            404,
            "NotFoundException",
            &format!("{case} / DeleteMethod again"),
        );

        // Every verb not yet deleted is untouched, and the resource reports exactly
        // those.
        remaining.retain(|kept| kept != verb);
        for kept in &remaining {
            let answer = get_method(&gateway, &api, &resource_id, kept).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: {kept} must survive, got {} with {:?}",
                answer.status, answer.body
            );
        }
        let reported = resource_verbs(&gateway, &api, &resource_id, &case).await;
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected resourceMethods {expected:?}, got {reported:?}"
        );
    }

    // The verb is free again once its method is gone: re-putting it is no conflict,
    // and it reads back.
    for verb in HTTP_METHODS {
        put_method(&gateway, &api, &resource_id, verb).await;
        let answer = get_method(&gateway, &api, &resource_id, verb).await;
        assert_eq!(
            answer.body["httpMethod"].as_str(),
            Some(*verb),
            "case PutMethod({verb}) after delete: expected the verb back, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn deleting_a_resource_removes_it_with_its_whole_subtree() {
    // Rows differ along what the rule generalises over: the shape of the subtree the
    // deleted resource owns — none, a chain, a branching tree of two levels — and
    // whether the target sits directly under the root or further down.
    let cases: &[(&str, &[&str], &str, &[&str])] = &[
        ("a leaf under the root", &["/pets", "/owners"], "/pets", &["/pets"]),
        (
            "a node with one child",
            &["/pets", "/pets/{petId}", "/owners"],
            "/pets",
            &["/pets", "/pets/{petId}"],
        ),
        (
            "a branching two-level subtree",
            &[
                "/pets",
                "/pets/{petId}",
                "/pets/{petId}/toys",
                "/pets/search",
                "/owners",
                "/owners/{ownerId}",
            ],
            "/pets",
            &["/pets", "/pets/{petId}", "/pets/{petId}/toys", "/pets/search"],
        ),
        (
            "a mid node, leaving its parent",
            &["/pets", "/pets/{petId}", "/pets/{petId}/toys", "/owners"],
            "/pets/{petId}",
            &["/pets/{petId}", "/pets/{petId}/toys"],
        ),
    ];

    for (case, paths, target, removed) in cases {
        let gateway = Gateway::new();
        let (api, root) = create_api(&gateway, "resource-deletes").await;
        let ids = build_tree(&gateway, &api, &root, paths).await;
        let target_id = ids
            .get(*target)
            .unwrap_or_else(|| panic!("case {case}: {target} is part of the tree, got {ids:?}"));
        // A method on the target, so the delete is shown to take the records the
        // resource owns with it.
        put_method(&gateway, &api, target_id, "GET").await;

        let deleted = gateway
            .call("DELETE", &format!("/restapis/{api}/resources/{target_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the modelled 202, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {case}: expected an empty body, got {:?}",
            deleted.body
        );

        // Every resource of the deleted subtree is gone for a reader, and deleting it
        // again finds nothing to delete.
        for path in *removed {
            let id = ids
                .get(*path)
                .unwrap_or_else(|| panic!("case {case}: {path} is part of the tree, got {ids:?}"));
            for (method, uri) in [
                ("GET", format!("/restapis/{api}/resources/{id}")),
                ("DELETE", format!("/restapis/{api}/resources/{id}")),
                (
                    "GET",
                    format!("/restapis/{api}/resources/{id}/methods/GET"),
                ),
                (
                    "POST",
                    format!("/restapis/{api}/resources/{id}"),
                ),
            ] {
                let probe = format!("{case} / {method} {uri}");
                let body = (method == "POST").then(|| json!({ "pathPart": "x" }));
                assert_failure(
                    &gateway.call(method, &uri, body).await,
                    404,
                    "NotFoundException",
                    &probe,
                );
            }
        }

        // What the delete does not own is untouched: the listing reports exactly the
        // root plus the paths outside the deleted subtree.
        let mut expected: Vec<String> = std::iter::once("/".to_string())
            .chain(
                paths
                    .iter()
                    .filter(|path| !removed.contains(path))
                    .map(|path| (*path).to_string()),
            )
            .collect();
        expected.sort();
        let listed = listed_paths(&gateway, &api, case).await;
        assert_eq!(
            listed, expected,
            "case {case}: expected the surviving paths {expected:?}, got {listed:?}"
        );

        // The freed path segment is available again under the same parent: nothing
        // stale is left to conflict with it.
        let (parent_path, part) = target
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("case {case}: {target} carries a separator"));
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let parent_id = ids.get(parent_path).unwrap_or_else(|| {
            panic!("case {case}: the parent {parent_path} is part of the tree, got {ids:?}")
        });
        let recreated = create_resource(&gateway, &api, parent_id, part).await;
        assert_eq!(
            recreated["path"].as_str(),
            Some(*target),
            "case {case}: expected the freed path back, got {:?}",
            recreated["path"]
        );
    }
}

#[tokio::test]
async fn the_root_resource_goes_only_with_its_api() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "root-delete").await;
    let paths = ["/pets", "/pets/{petId}"];
    build_tree(&gateway, &api, &root, &paths).await;

    let refused = gateway
        .call("DELETE", &format!("/restapis/{api}/resources/{root}"), None)
        .await;
    assert_failure(
        &refused,
        400,
        "BadRequestException",
        "DeleteResource(root resource)",
    );

    // A refused delete changes nothing.
    let listed = listed_paths(&gateway, &api, "after the refused root delete").await;
    assert_eq!(
        listed,
        vec!["/", "/pets", "/pets/{petId}"],
        "case DeleteResource(root resource): the tree must be unchanged, got {listed:?}"
    );

    // Deleting the API takes the root with it.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &gateway
            .call("GET", &format!("/restapis/{api}/resources/{root}"), None)
            .await,
        404,
        "NotFoundException",
        "GetResource(root of a deleted API)",
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_both_delete_operations() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "delete-notfound").await;
    let resource = create_resource(&gateway, &api, &root, "pets").await;
    let resource_id = string_member(&resource, "id", "delete-notfound/pets");
    put_method(&gateway, &api, &resource_id, "GET").await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier the two operations resolve, one unknown at a time:
    // the API, the resource, and the verb naming no method on the resource (including
    // one outside the verb catalog).
    let cases: Vec<String> = vec![
        format!("/restapis/{missing}/resources/{resource_id}"),
        format!("/restapis/{api}/resources/{missing}"),
        format!("/restapis/{missing}/resources/{resource_id}/methods/GET"),
        format!("/restapis/{api}/resources/{missing}/methods/GET"),
        format!("/restapis/{api}/resources/{resource_id}/methods/PUT"),
        format!("/restapis/{api}/resources/{resource_id}/methods/TRACE"),
    ];

    for uri in cases {
        let case = format!("DELETE {uri}");
        assert_failure(
            &gateway.call("DELETE", &uri, None).await,
            404,
            "NotFoundException",
            &case,
        );
    }

    // The records the refused calls named are still there.
    let answer = get_method(&gateway, &api, &resource_id, "GET").await;
    assert_eq!(
        answer.status, 200,
        "case GetMethod after refused deletes: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}
