// SPDX-License-Identifier: CC0-1.0

//! Deleting an address list, end to end over the wire.
//!
//! RULE: `DeleteAddressList` removes the stored address list the request names, with the
//! membership that lives inside it, and frees its `AddressListName`; afterwards every
//! operation that resolves that `AddressListId` — including a read of an import job the
//! deletion ORPHANED, whose `AddressListId` names the list it was created for — answers the
//! refusal this service gives an address-list id it cannot resolve: HTTP 400, the
//! `ResourceNotFoundException` code and "Could not find AddressList with id <that id>".
//!
//! Every assertion goes through the real request path (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target: MailManagerSvc.<Operation>`, and a plain
//! `PUT` of the published upload URL for an input file), so what it pins is the emulator's
//! HTTP behaviour rather than an internal helper.
//!
//! The tests are tables: the rows span `AddressListName`'s modelled pattern and both length
//! bounds, memberships of several sizes, both `ImportDataType` values, lists with and
//! without an import job, and ids that resolve to nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// The endpoint host the test addresses; the upload URL this service hands out names it.
const HOST: &str = "mail-manager.localhost:9297";

/// One emulator holding one MailManager service on one shared engine.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, and the fixed test credential below
        // is not this process's signing key.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one modelled operation and return `(http status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("MailManagerSvc.{operation}"))
            // A signed SDK request routes by its SigV4 credential scope, whose 4th field is
            // this service's signing name (`ses`) and whose 3rd is the region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/ses/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a valid HTTP request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `PUT` `body` to an import job's published upload URL and return `(status, body)`.
    async fn upload(&self, url: &str, body: &str) -> (u16, Value) {
        let path = url
            .split_once(HOST)
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| url.to_string());
        let request = http::Request::builder()
            .method("PUT")
            .uri(&path)
            .header("host", HOST)
            .body(bytes::Bytes::from(body.to_owned()))
            .expect("a valid HTTP request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            json!({})
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create an address list and return its `AddressListId`, failing the test on a refusal.
    async fn create_list(&self, body: Value) -> String {
        let (status, out) = self.call("CreateAddressList", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {body:?}: expected CreateAddressList to answer 200, got {status} {out:?}"
        );
        out["AddressListId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {body:?}: no AddressListId in {out:?}"))
            .to_string()
    }

    /// Create an import job on `address_list_id` and return `(JobId, PreSignedUrl)`.
    async fn create_job(&self, address_list_id: &str, name: &str, data_type: &str) -> (String, String) {
        let (status, out) = self
            .call(
                "CreateAddressListImportJob",
                json!({
                    "AddressListId": address_list_id,
                    "Name": name,
                    "ImportDataFormat": { "ImportDataType": data_type },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateAddressListImportJob to answer 200, got {status} \
             {out:?}"
        );
        (
            out["JobId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: no JobId in {out:?}"))
                .to_string(),
            out["PreSignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: no PreSignedUrl in {out:?}"))
                .to_string(),
        )
    }

    /// Register `address` as a member of `address_list_id`, failing the test on a refusal.
    async fn register(&self, address_list_id: &str, address: &str) {
        let (status, out) = self
            .call(
                "RegisterMemberToAddressList",
                json!({ "AddressListId": address_list_id, "Address": address }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {address:?}: expected RegisterMemberToAddressList to answer 200, got {status} \
             {out:?}"
        );
    }

    /// Delete `address_list_id`, asserting the answer is the empty success the model declares.
    async fn delete_list(&self, case: &str, address_list_id: &str) {
        let (status, out) = self
            .call(
                "DeleteAddressList",
                json!({ "AddressListId": address_list_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteAddressList to answer 200, got {status} {out:?}"
        );
        assert_eq!(
            out,
            json!({}),
            "case {case:?}: DeleteAddressListResponse has no members, got {out:?}"
        );
    }
}

/// The wire error code of a failure body: the `__type` member, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .or_else(|| body.get("Code").and_then(Value::as_str))
        .unwrap_or("<no __type>")
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// The message of a failure body, whichever casing the plane that answered uses.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert one answer is this service's refusal of an `AddressListId` it cannot resolve.
fn assert_address_list_missing(case: &str, status: u16, body: &Value, address_list_id: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: an unresolvable AddressListId must answer 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: unexpected error code, got {body:?}"
    );
    assert_eq!(
        error_message(body),
        format!("Could not find AddressList with id {address_list_id}"),
        "case {case:?}: unexpected refusal message, got {body:?}"
    );
}

/// RULE: a deleted address list is gone — the record, its membership and the name it held —
/// and every operation that resolves its id afterwards answers the unresolvable-id refusal.
///
/// The rows span `AddressListName`'s modelled pattern `^[a-zA-Z0-9_.-]+$` and both ends of
/// its 1..255 length (`smithy_query(query="shape",shape="AddressListName")`), crossed with
/// memberships of 0, 1 and several addresses and with tags present or absent.
#[tokio::test]
async fn deleting_an_address_list_removes_it_and_the_membership_it_holds() {
    let harness = Harness::new();

    // (case, name, the addresses registered before the delete)
    let rows: [(&str, String, Vec<&str>); 5] = [
        ("the shortest name, no members", "a".to_string(), vec![]),
        (
            "an underscored name, one member",
            "with_underscore".to_string(),
            vec!["user@example.com"],
        ),
        (
            "a dotted name, several members",
            "with.dots.1".to_string(),
            vec!["one@example.com", "two@example.org", "example.net"],
        ),
        (
            "a mixed-case, hyphenated name, one domain member",
            "Mixed-Case.2".to_string(),
            vec!["example.com"],
        ),
        (
            "the longest name the length admits, no members",
            "z".repeat(255),
            vec![],
        ),
    ];

    for (case, name, addresses) in rows {
        let mut body = json!({ "AddressListName": name });
        if !addresses.is_empty() {
            body["Tags"] = json!([{ "Key": "team", "Value": "mail" }]);
        }
        let address_list_id = harness.create_list(body).await;
        for address in &addresses {
            harness.register(&address_list_id, address).await;
        }

        harness.delete_list(case, &address_list_id).await;

        // Every operation that resolves an `AddressListId` now refuses this one the same
        // way: the list is gone, not emptied.
        let calls: [(&str, Value); 5] = [
            ("GetAddressList", json!({ "AddressListId": &address_list_id })),
            (
                "ListMembersOfAddressList",
                json!({ "AddressListId": &address_list_id }),
            ),
            (
                "GetMemberOfAddressList",
                json!({ "AddressListId": &address_list_id, "Address": "user@example.com" }),
            ),
            (
                "RegisterMemberToAddressList",
                json!({ "AddressListId": &address_list_id, "Address": "user@example.com" }),
            ),
            (
                "CreateAddressListImportJob",
                json!({
                    "AddressListId": &address_list_id,
                    "Name": "after.delete",
                    "ImportDataFormat": { "ImportDataType": "CSV" },
                }),
            ),
        ];
        for (operation, request) in calls {
            let row = format!("{case} / {operation} after the delete");
            let (status, out) = harness.call(operation, request).await;
            assert_address_list_missing(&row, status, &out, &address_list_id);
        }

        // The name the deleted list held is free again, and the list that takes it is a
        // DIFFERENT, empty one: the membership did not survive its list.
        let recreated = harness
            .create_list(json!({ "AddressListName": name }))
            .await;
        assert_ne!(
            recreated, address_list_id,
            "case {case:?}: recreating the freed name must mint a fresh AddressListId"
        );
        let (status, members) = harness
            .call("ListMembersOfAddressList", json!({ "AddressListId": &recreated }))
            .await;
        assert_eq!(
            (status, members["Addresses"].as_array().map(Vec::len)),
            (200, Some(0)),
            "case {case:?}: the recreated address list must hold no members, got {status} \
             {members:?}"
        );
    }
}

/// RULE (the orphan): the import jobs created against a deleted address list keep their own
/// `JobId`, and every read or upload that reaches one is refused for the list it names,
/// which no longer resolves.
///
/// The rows cross both `ImportDataType` values the model declares
/// (`smithy_query(query="shape",shape="ImportDataType")`: `CSV`, `JSON`) with jobs that have
/// and have not had an input file uploaded.
#[tokio::test]
async fn an_import_job_orphaned_by_the_delete_is_refused_for_its_missing_list() {
    let harness = Harness::new();

    // (case, ImportDataType, whether a file is uploaded before the delete)
    let rows: [(&str, &str, bool); 4] = [
        ("CSV, nothing uploaded", "CSV", false),
        ("CSV, a file uploaded", "CSV", true),
        ("JSON, nothing uploaded", "JSON", false),
        ("JSON, a file uploaded", "JSON", true),
    ];

    for (index, (case, data_type, uploaded)) in rows.into_iter().enumerate() {
        let address_list_id = harness
            .create_list(json!({ "AddressListName": format!("orphan-{index}") }))
            .await;
        let (job_id, upload_url) = harness
            .create_job(&address_list_id, &format!("orphan.job.{index}"), data_type)
            .await;

        if uploaded {
            let (status, out) = harness.upload(&upload_url, "user@example.com\n").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the upload target must serve a live job's URL, got {status} \
                 {out:?}"
            );
        }

        // Before the delete the job reads back, naming the list it was created for.
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": &job_id }))
            .await;
        assert_eq!(
            (status, out["AddressListId"].as_str()),
            (200, Some(address_list_id.as_str())),
            "case {case:?}: a live job must report the list it was created for, got {status} \
             {out:?}"
        );

        harness.delete_list(case, &address_list_id).await;

        // After it, the job is orphaned: the read is refused for the list, not served.
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": &job_id }))
            .await;
        assert_address_list_missing(
            &format!("{case} / GetAddressListImportJob on the orphaned job"),
            status,
            &out,
            &address_list_id,
        );

        // So is a file uploaded to the URL that job was handed: nothing would import it.
        let (status, out) = harness.upload(&upload_url, "late@example.com\n").await;
        assert_eq!(
            status, 404,
            "case {case:?}: an orphaned job's upload URL must be refused, got {status} {out:?}"
        );
        assert_eq!(
            error_message(&out),
            format!("Could not find AddressList with id {address_list_id}"),
            "case {case:?}: unexpected upload refusal message, got {out:?}"
        );
    }
}

/// RULE: `DeleteAddressList` resolves its `AddressListId` like every other address-list
/// operation, so an id that names no stored list — including one a delete has already
/// removed — is refused with HTTP 400, `ResourceNotFoundException` and the wording naming
/// that id, and the refusal deletes nothing else.
#[tokio::test]
async fn deleting_an_unresolvable_address_list_id_is_refused() {
    let harness = Harness::new();
    let live = harness
        .create_list(json!({ "AddressListName": "survives.every.refusal" }))
        .await;
    let doomed = harness
        .create_list(json!({ "AddressListName": "deleted.once" }))
        .await;
    harness.delete_list("the first delete", &doomed).await;

    // Rows: ids inside `AddressListId`'s modelled pattern `^[a-zA-Z0-9-]+$` and 1..255
    // length that name nothing — the shortest, a hyphenated one, the longest, the live id
    // with one character changed, and the id of a list already deleted (a repeat delete).
    let rows: [(&str, String); 5] = [
        ("the shortest id the length admits", "x".to_string()),
        ("a hyphenated id", "no-such-address-list".to_string()),
        ("the longest id the length admits", "9".repeat(255)),
        (
            "the live id with its last character changed",
            format!(
                "{}{}",
                &live[..live.len() - 1],
                if live.ends_with('0') { '1' } else { '0' }
            ),
        ),
        ("an id this test already deleted", doomed.clone()),
    ];

    for (row, id) in rows {
        assert_ne!(id, live, "case {row:?}: the row must not name the live list");
        let (status, out) = harness
            .call("DeleteAddressList", json!({ "AddressListId": &id }))
            .await;
        assert_address_list_missing(&format!("DeleteAddressList with {row}"), status, &out, &id);
    }

    // The list that DOES resolve is untouched by every refusal above.
    let (status, out) = harness
        .call("GetAddressList", json!({ "AddressListId": &live }))
        .await;
    assert_eq!(
        (status, out["AddressListId"].as_str()),
        (200, Some(live.as_str())),
        "case live-after-refusals: the stored address list must still resolve, got {status} \
         {out:?}"
    );
}

/// RULE: the delete takes only the list it names, so the other lists, their members and
/// their import jobs are all still resolvable afterwards.
#[tokio::test]
async fn deleting_one_address_list_leaves_the_others_alone() {
    let harness = Harness::new();

    // Rows: several lists created side by side, each with its own member and import job;
    // one of them is deleted.
    let mut lists: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let id = harness
            .create_list(json!({ "AddressListName": format!("neighbour-{index}") }))
            .await;
        harness
            .register(&id, &format!("member{index}@example.com"))
            .await;
        let (job_id, _) = harness
            .create_job(&id, &format!("neighbour.job.{index}"), "CSV")
            .await;
        lists.push((id, job_id));
    }

    let (deleted, deleted_job) = lists.remove(1);
    harness.delete_list("the middle list", &deleted).await;

    for (index, (id, job_id)) in lists.iter().enumerate() {
        let case = format!("neighbour {index} after the delete");
        let (status, out) = harness
            .call("GetAddressList", json!({ "AddressListId": id }))
            .await;
        assert_eq!(
            (status, out["AddressListId"].as_str()),
            (200, Some(id.as_str())),
            "case {case:?}: the list must still resolve, got {status} {out:?}"
        );
        let (status, out) = harness
            .call("ListMembersOfAddressList", json!({ "AddressListId": id }))
            .await;
        assert_eq!(
            (status, out["Addresses"].as_array().map(Vec::len)),
            (200, Some(1)),
            "case {case:?}: the list must keep its member, got {status} {out:?}"
        );
        let (status, out) = harness
            .call("GetAddressListImportJob", json!({ "JobId": job_id }))
            .await;
        assert_eq!(
            (status, out["JobId"].as_str()),
            (200, Some(job_id.as_str())),
            "case {case:?}: the list's import job must still resolve, got {status} {out:?}"
        );
    }

    // Only the deleted list's own job is orphaned.
    let (status, out) = harness
        .call("GetAddressListImportJob", json!({ "JobId": &deleted_job }))
        .await;
    assert_address_list_missing(
        "the deleted list's own import job",
        status,
        &out,
        &deleted,
    );
}

/// RULE: a `ClientToken` is a retry of a create, not a handle on a deleted resource — once
/// the list it created is gone, the same token creates a new, resolvable list rather than
/// answering with the id of one nothing can resolve.
#[tokio::test]
async fn a_client_token_whose_address_list_was_deleted_creates_a_new_one() {
    let harness = Harness::new();

    // Rows vary the request the token travels with: a bare create and one carrying tags.
    let rows: [(&str, Value); 2] = [
        ("a bare create", json!({ "AddressListName": "token.bare" })),
        (
            "a create with tags",
            json!({
                "AddressListName": "token.tagged",
                "Tags": [{ "Key": "env", "Value": "test" }],
            }),
        ),
    ];

    for (index, (case, base)) in rows.into_iter().enumerate() {
        let mut body = base.clone();
        body["ClientToken"] = json!(format!("token-{index}"));

        let first = harness.create_list(body.clone()).await;
        // The same token, the same request: a retry, answered with the first id.
        let retried = harness.create_list(body.clone()).await;
        assert_eq!(
            retried, first,
            "case {case:?}: a retry before the delete must return the original id"
        );

        harness.delete_list(case, &first).await;

        let after = harness.create_list(body).await;
        assert_ne!(
            after, first,
            "case {case:?}: the token must not answer with the id of a deleted list"
        );
        let (status, out) = harness
            .call("GetAddressList", json!({ "AddressListId": &after }))
            .await;
        assert_eq!(
            (status, out["AddressListId"].as_str()),
            (200, Some(after.as_str())),
            "case {case:?}: the id the retry answered with must resolve, got {status} {out:?}"
        );
    }
}
