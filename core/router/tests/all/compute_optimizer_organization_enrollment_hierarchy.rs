// SPDX-License-Identifier: CC0-1.0

//! Compute Optimizer Automation's ORGANIZATION ENROLLMENT CONFIGURATIONS over the
//! organization hierarchy AWS Organizations owns.
//!
//! RULE: an organization enrollment configuration stored on one target — the organization
//! root, an organizational unit (OU) or an account — supplies the effective organization
//! rule mode of every node it covers: the configuration on the node itself (`Applied`),
//! else the one on the closest parent that has one (`Inherited`), else None allowed
//! (`Default`); a node a configuration applies to has Automation enabled for the member
//! accounts beneath it (`autoEnrollMemberAccounts: All`) whichever of the two rule modes
//! the configuration names, and one no configuration applies to does not (`None`).
//! Deleting a configuration leaves its target taking its mode from the closest remaining
//! parent, or None allowed when none has one. A configuration cannot be created for the
//! management account.
//!
//! Only a cross-service suite can pin this: the root and the OUs are records the
//! Organizations control plane owns — `ListOrganizationalUnits` reports the nodes THAT
//! service holds and nothing while the account is in no organization — while the
//! configurations and the resolution are Compute Optimizer Automation's. Both services are
//! mounted on ONE engine, exactly as `core/router/src/lib.rs` mounts them, and every
//! assertion goes over the wire each service publishes (`awsJson1_1` with an `X-Amz-Target`
//! for Organizations, `awsJson1_0` with one for Compute Optimizer Automation).
//!
//! The rows span what the rule generalises over: a hierarchy of one node and of three at
//! two levels below the root, a configuration on the root and on an OU under it, both
//! `OrganizationRuleMode` values, the node the configuration sits on and the nodes one and
//! two levels below it, the whole life cycle create → put → put → delete, and the request
//! that selects a subset of the nodes by id.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region both control planes are addressed in. Organizations is global and the
/// configurations are organization-wide, so the Region selects nothing; one is needed
/// because the credential scope is what routes a request.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix AWS Organizations routes on (that crate's `OPERATIONS.md`).
const ORGANIZATIONS_TARGET_PREFIX: &str = "AWSOrganizationsV20161128";

/// The `X-Amz-Target` prefix Compute Optimizer Automation routes on, and the signing name
/// its credential scope carries (that crate's `OPERATIONS.md`).
const AUTOMATION_TARGET_PREFIX: &str = "ComputeOptimizerAutomationService";
const AUTOMATION_SIGNING_NAME: &str = "aco-automation";

/// The one `OrganizationEnrollmentConfigurationType` the model declares
/// (`smithy_query(query="shape",shape="OrganizationEnrollmentConfigurationType")`), spelled
/// out here rather than imported so a wrong value in the implementation cannot make this
/// suite pass.
const CONFIGURATION_TYPE: &str = "OrganizationRuleMode";

/// The `OrganizationRuleMode` values, from
/// `smithy_query(query="shape",shape="OrganizationRuleMode")`.
const ANY_ALLOWED: &str = "AnyAllowed";
const NONE_ALLOWED: &str = "NoneAllowed";

/// The `AutoEnrollMemberAccounts` values, from
/// `smithy_query(query="shape",shape="AutoEnrollMemberAccounts")`.
const AUTO_ENROLL_ALL: &str = "All";
const AUTO_ENROLL_NONE: &str = "None";

/// The `OrganizationRuleModeSourceType` values, from
/// `smithy_query(query="shape",shape="OrganizationRuleModeSourceType")`.
const APPLIED: &str = "Applied";
const INHERITED: &str = "Inherited";
const DEFAULT: &str = "Default";

/// What a node must report: its rule mode, how it is sourced, and from which target.
type Expected<'a> = (&'a str, &'a str, Option<&'a str>, &'a str);

/// What the emulator answered: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code, taken off `__type`, or the empty string on a success.
    fn error_code(&self) -> &str {
        let code = self.body["__type"].as_str().unwrap_or_default();
        code.rsplit('#').next().unwrap_or_default()
    }

    /// One string member of the body, or the empty string when it is absent.
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }
}

/// Compute Optimizer Automation and AWS Organizations on one engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// Both services mounted on one fresh engine, with the caller opted in to Compute
    /// Optimizer Automation — the one enrollment status under which that service performs
    /// an action — established over its own wire.
    async fn mounted() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These callers sign nothing, so they must not be refused for it; the credential
        // scope below is what routes each request and names its Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
                engine,
            ),
        );
        let emulator = Self { router };
        emulator
            .automation_ok(
                "UpdateEnrollmentConfiguration",
                &json!({"status": "Active"}),
                "opt-in",
            )
            .await;
        emulator
    }

    /// Dispatch `request` and read the answer off it.
    async fn dispatch(&self, called: &str, request: ::http::Request<Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {called:?}: the answer is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// One AWS Organizations `awsJson1_1` call, routed by its `X-Amz-Target`.
    async fn organizations(&self, operation: &str, body: &Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("organizations.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{ORGANIZATIONS_TARGET_PREFIX}.{operation}"),
            )
            .header("authorization", credential_scope("organizations"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(operation, request).await
    }

    /// One AWS Organizations call, required to succeed.
    async fn organizations_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let answer = self.organizations(operation, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {operation} to be served, got {} with {}",
            answer.status, answer.body
        );
        answer.body
    }

    /// One Compute Optimizer Automation `awsJson1_0` call, routed by its `X-Amz-Target`.
    async fn automation(&self, operation: &str, body: &Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("aco-automation.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("{AUTOMATION_TARGET_PREFIX}.{operation}"),
            )
            .header("authorization", credential_scope(AUTOMATION_SIGNING_NAME))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(operation, request).await
    }

    /// One Compute Optimizer Automation call, required to succeed.
    async fn automation_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let answer = self.automation(operation, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {operation} to be served, got {} with {}",
            answer.status, answer.body
        );
        answer.body
    }

    /// The organization root's id, read off Organizations' own `ListRoots`.
    async fn root_id(&self, case: &str) -> String {
        let roots = self.organizations_ok("ListRoots", &json!({}), case).await;
        let id = roots["Roots"][0]["Id"].as_str().unwrap_or_default().to_string();
        assert!(
            id.starts_with("r-"),
            "case {case:?}: expected a RootId matching ^r-[0-9a-z]{{4,32}}$, got {roots}"
        );
        id
    }

    /// The organization's management account id, read off Organizations' own
    /// `DescribeOrganization`.
    async fn management_account_id(&self, case: &str) -> String {
        let described = self
            .organizations_ok("DescribeOrganization", &json!({}), case)
            .await;
        let id = described["Organization"]["MasterAccountId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            id.len() == 12,
            "case {case:?}: expected the management account id, got {described}"
        );
        id
    }

    /// Create an OU named `name` under `parent_id` and answer its id.
    async fn create_organizational_unit(&self, parent_id: &str, name: &str) -> String {
        let created = self
            .organizations_ok(
                "CreateOrganizationalUnit",
                &json!({"ParentId": parent_id, "Name": name}),
                name,
            )
            .await;
        let id = created["OrganizationalUnit"]["Id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            id.starts_with("ou-"),
            "case {name:?}: expected an OrganizationalUnitId, got {created}"
        );
        id
    }

    /// Every node `ListOrganizationalUnits` reports, walked to the end so no case depends
    /// on a page size. `ids` is the request's `organizationalUnitIds`.
    async fn list_nodes(&self, ids: Option<&[String]>, case: &str) -> Vec<Value> {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(ids) = ids {
                request["organizationalUnitIds"] = json!(ids);
            }
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = self
                .automation_ok("ListOrganizationalUnits", &request, case)
                .await;
            walked.extend(
                page["organizationalUnits"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= 64,
                "case {case:?}: the walk must terminate, walked {} nodes",
                walked.len()
            );
        }
        walked
    }

    /// Store `mode` on the OU or root `target_id`.
    async fn put_on_node(&self, target_id: &str, mode: &str, case: &str) {
        self.automation_ok(
            "PutOrganizationEnrollmentConfiguration",
            &json!({
                "target": {"organizationalUnitId": target_id},
                "organizationRuleMode": mode,
            }),
            case,
        )
        .await;
    }

    /// How many rows a paginated listing reports for `request`, walked to the
    /// end so no case depends on a page size.
    async fn count_rows(&self, operation: &str, member: &str, request: &Value, case: &str) -> usize {
        let mut counted = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut page_request = request.clone();
            if let Some(token) = &token {
                page_request["nextToken"] = json!(token);
            }
            let page = self.automation_ok(operation, &page_request, case).await;
            counted += page[member].as_array().map(Vec::len).unwrap_or_default();
            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                return counted;
            }
            assert!(
                counted <= 1024,
                "case {case:?}: the walk must terminate, counted {counted} rows"
            );
        }
    }

    /// The total count the ONE aggregate row of a summary listing reports, read
    /// through the member path `total_path` names.
    async fn summary_count(
        &self,
        operation: &str,
        member: &str,
        total_member: &str,
        request: &Value,
        case: &str,
    ) -> i64 {
        let page = self.automation_ok(operation, request, case).await;
        page[member]
            .as_array()
            .and_then(|rows| rows.first().cloned())
            .and_then(|row| row["total"][total_member].as_i64())
            .unwrap_or_default()
    }

    /// Remove the configuration from the OU or root `target_id`.
    async fn delete_on_node(&self, target_id: &str, case: &str) {
        self.automation_ok(
            "DeleteOrganizationEnrollmentConfiguration",
            &json!({
                "target": {"organizationalUnitId": target_id},
                "configurationTypes": [CONFIGURATION_TYPE],
            }),
            case,
        )
        .await;
    }

    /// The target ids `ListOrganizationEnrollmentConfigurations` reports as STORED.
    async fn stored_target_ids(&self, case: &str) -> Vec<String> {
        let listed = self
            .automation_ok(
                "ListOrganizationEnrollmentConfigurations",
                &json!({"maxResults": 1000}),
                case,
            )
            .await;
        listed["organizationEnrollmentConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| row["targetId"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` in [`REGION`]
/// — the signal the router resolves a request's service from. The signature is never
/// checked: this suite runs with request authentication off.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// Assert the listed nodes are exactly `expected`, keyed by node id, each reporting the
/// rule mode, source type, source target and auto-enroll value the rule gives it.
fn assert_nodes(listed: &[Value], expected: &[(String, Expected<'_>)], case: &str) {
    let reported: Vec<String> = listed
        .iter()
        .map(|node| {
            node["organizationalUnitId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let wanted: Vec<String> = expected.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        reported, wanted,
        "case {case:?}: the listing reports the organization's nodes, root first and then the \
         OUs in one stable order, got {listed:?}"
    );
    for (id, (mode, source_type, source_target_id, auto_enroll)) in expected {
        let node = listed
            .iter()
            .find(|node| node["organizationalUnitId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be listed, got {listed:?}"));
        assert_eq!(
            node["organizationRuleMode"].as_str(),
            Some(*mode),
            "case {case:?}: {id} must report the effective rule mode {mode}, got {node}"
        );
        assert_eq!(
            node["organizationRuleModeSource"]["sourceType"].as_str(),
            Some(*source_type),
            "case {case:?}: {id} must report sourceType {source_type}, got {node}"
        );
        assert_eq!(
            node["organizationRuleModeSource"]["sourceTargetId"].as_str(),
            *source_target_id,
            "case {case:?}: {id} must report sourceTargetId {source_target_id:?}, got {node}"
        );
        assert_eq!(
            node["autoEnrollMemberAccounts"].as_str(),
            Some(*auto_enroll),
            "case {case:?}: {id} must report autoEnrollMemberAccounts {auto_enroll}, got {node}"
        );
    }
}

/// The nodes in the order the listing reports them: the root, then the OU ids ascending.
fn in_listing_order(root: &str, ous: &[String]) -> Vec<String> {
    let mut ordered: Vec<String> = ous.to_vec();
    ordered.sort();
    let mut all = vec![root.to_string()];
    all.extend(ordered);
    all
}

/// Before any `CreateOrganization` the account is in no organization, so the listing
/// reports no node at all — there is no root and no OU to report — and the create is what
/// makes the root appear, at the default rule mode nothing has configured.
#[tokio::test]
async fn the_listing_reports_the_nodes_organizations_holds_and_nothing_before_the_create() {
    let emulator = Emulator::mounted().await;

    let before = emulator.list_nodes(None, "before-the-create").await;
    assert!(
        before.is_empty(),
        "case \"before-the-create\": an account in no organization has no root and no OU, got \
         {before:?}"
    );

    emulator
        .organizations_ok("CreateOrganization", &json!({}), "create")
        .await;
    let root = emulator.root_id("create").await;

    let after = emulator.list_nodes(None, "after-the-create").await;
    assert_nodes(
        &after,
        &[(
            root.clone(),
            (NONE_ALLOWED, DEFAULT, None, AUTO_ENROLL_NONE),
        )],
        "after-the-create",
    );

    // Each OU Organizations creates joins the listing, still at the default.
    let first = emulator.create_organizational_unit(&root, "Sandbox").await;
    let second = emulator.create_organizational_unit(&first, "Nested").await;
    let expected: Vec<(String, Expected<'_>)> =
        in_listing_order(&root, &[first.clone(), second.clone()])
            .into_iter()
            .map(|id| (id, (NONE_ALLOWED, DEFAULT, None, AUTO_ENROLL_NONE)))
            .collect();
    assert_nodes(
        &emulator.list_nodes(None, "after-the-ous").await,
        &expected,
        "after-the-ous",
    );

    // The listing is a READ: reading it twice leaves the next caller the same answer.
    assert_eq!(
        emulator.list_nodes(None, "second-read").await,
        emulator.list_nodes(None, "third-read").await,
        "case \"second-read\": a read must leave the next caller the same answer"
    );
}

/// A configuration on a node supplies the rule mode of that node and of every node beneath
/// it, until a nearer one takes over; deleting it hands the covered nodes to the closest
/// remaining parent, or to the default when none has one.
#[tokio::test]
async fn a_configuration_is_applied_on_its_node_and_inherited_down_the_hierarchy() {
    let emulator = Emulator::mounted().await;
    emulator
        .organizations_ok("CreateOrganization", &json!({}), "create")
        .await;
    let root = emulator.root_id("create").await;
    let outer = emulator.create_organizational_unit(&root, "Outer").await;
    let inner = emulator.create_organizational_unit(&outer, "Inner").await;
    let order = in_listing_order(&root, &[outer.clone(), inner.clone()]);

    // The configuration on the ROOT covers both levels below it, and `AnyAllowed` enables
    // Automation for the member accounts beneath every covered node.
    emulator.put_on_node(&root, ANY_ALLOWED, "root-any-allowed").await;
    let expected: Vec<(String, Expected<'_>)> = order
        .iter()
        .map(|id| {
            let row: Expected<'_> = if id == &root {
                (ANY_ALLOWED, APPLIED, Some(root.as_str()), AUTO_ENROLL_ALL)
            } else {
                (ANY_ALLOWED, INHERITED, Some(root.as_str()), AUTO_ENROLL_ALL)
            };
            (id.clone(), row)
        })
        .collect();
    assert_nodes(
        &emulator.list_nodes(None, "root-any-allowed").await,
        &expected,
        "root-any-allowed",
    );

    // A NEARER configuration takes over for the node it is on and for the node below it,
    // while the root keeps its own. `NoneAllowed` still enables Automation.
    emulator.put_on_node(&outer, NONE_ALLOWED, "outer-none-allowed").await;
    let expected: Vec<(String, Expected<'_>)> = order
        .iter()
        .map(|id| {
            let row: Expected<'_> = if id == &root {
                (ANY_ALLOWED, APPLIED, Some(root.as_str()), AUTO_ENROLL_ALL)
            } else if id == &outer {
                (NONE_ALLOWED, APPLIED, Some(outer.as_str()), AUTO_ENROLL_ALL)
            } else {
                (NONE_ALLOWED, INHERITED, Some(outer.as_str()), AUTO_ENROLL_ALL)
            };
            (id.clone(), row)
        })
        .collect();
    assert_nodes(
        &emulator.list_nodes(None, "outer-none-allowed").await,
        &expected,
        "outer-none-allowed",
    );

    // Both configurations are the STORED ones, reported under the OU target type.
    let mut stored = emulator.stored_target_ids("stored").await;
    stored.sort();
    let mut wanted = vec![root.clone(), outer.clone()];
    wanted.sort();
    assert_eq!(
        stored, wanted,
        "case \"stored\": only the targets a put named carry a configuration"
    );

    // `organizationalUnitIds` selects a subset of the nodes, each still reporting the mode
    // the whole hierarchy gives it.
    let selected = emulator
        .list_nodes(Some(std::slice::from_ref(&inner)), "selected-subset")
        .await;
    assert_nodes(
        &selected,
        &[(
            inner.clone(),
            (NONE_ALLOWED, INHERITED, Some(outer.as_str()), AUTO_ENROLL_ALL),
        )],
        "selected-subset",
    );

    // Deleting the ROOT's configuration leaves the root with no mode of its own, while the
    // nearer configuration still covers the two nodes below it.
    emulator.delete_on_node(&root, "root-deleted").await;
    let expected: Vec<(String, Expected<'_>)> = order
        .iter()
        .map(|id| {
            let row: Expected<'_> = if id == &root {
                (NONE_ALLOWED, DEFAULT, None, AUTO_ENROLL_NONE)
            } else if id == &outer {
                (NONE_ALLOWED, APPLIED, Some(outer.as_str()), AUTO_ENROLL_ALL)
            } else {
                (NONE_ALLOWED, INHERITED, Some(outer.as_str()), AUTO_ENROLL_ALL)
            };
            (id.clone(), row)
        })
        .collect();
    assert_nodes(
        &emulator.list_nodes(None, "root-deleted").await,
        &expected,
        "root-deleted",
    );

    // And deleting the last one leaves every node at the default.
    emulator.delete_on_node(&outer, "outer-deleted").await;
    let expected: Vec<(String, Expected<'_>)> = order
        .iter()
        .map(|id| (id.clone(), (NONE_ALLOWED, DEFAULT, None, AUTO_ENROLL_NONE)))
        .collect();
    assert_nodes(
        &emulator.list_nodes(None, "outer-deleted").await,
        &expected,
        "outer-deleted",
    );
    assert!(
        emulator.stored_target_ids("all-deleted").await.is_empty(),
        "case \"all-deleted\": no target carries a configuration any more"
    );
}

/// A configuration cannot be created for the management account — the account the
/// organization itself names as its master account — and the refused put stores nothing,
/// while the same call for any other account is served.
#[tokio::test]
async fn a_configuration_cannot_be_created_for_the_management_account() {
    let emulator = Emulator::mounted().await;
    emulator
        .organizations_ok("CreateOrganization", &json!({}), "create")
        .await;
    let management = emulator.management_account_id("create").await;

    let answer = emulator
        .automation(
            "PutOrganizationEnrollmentConfiguration",
            &json!({
                "target": {"accountId": management},
                "organizationRuleMode": ANY_ALLOWED,
            }),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "InvalidParameterValueException"),
        "case \"the-management-account\": expected the modelled 400, got {} with {}",
        answer.status,
        answer.body
    );
    assert!(
        answer.member("message").contains(&management),
        "case \"the-management-account\": the refusal must name the target it is about, got {:?}",
        answer.body["message"]
    );
    assert!(
        emulator
            .stored_target_ids("after-the-refusal")
            .await
            .is_empty(),
        "case \"after-the-refusal\": a refused put stores nothing"
    );

    // The management account's own enrollment read omits
    // `organizationEnrollmentConfigurationStatus`: the member "is returned for organization
    // member accounts, including delegated administrators. It's omitted for management
    // accounts and accounts that aren't in an organization".
    let read = emulator
        .automation_ok("GetEnrollmentConfiguration", &json!({}), "the-management-account")
        .await;
    assert_eq!(
        read.get("organizationEnrollmentConfigurationStatus"),
        None,
        "case \"the-management-account\": the member is omitted for a management account, got \
         {read}"
    );

    // The positive control: any other account is served, so the refusal keys on being the
    // management account rather than on the operation.
    let member_account = "111111111111";
    emulator
        .automation_ok(
            "PutOrganizationEnrollmentConfiguration",
            &json!({
                "target": {"accountId": member_account},
                "organizationRuleMode": ANY_ALLOWED,
            }),
            "a-member-account",
        )
        .await;
    assert_eq!(
        emulator.stored_target_ids("a-member-account").await,
        vec![member_account.to_string()],
        "case \"a-member-account\": the served put is the one that stored a configuration"
    );
}

/// The `organizationalUnitIds` restriction the four organization-wide listings take is
/// resolved through the organization: a row survives when its account sits in one of the
/// named OUs or organization roots, or in a child OU of one, and a request naming none
/// restricts nothing.
///
/// The rows span the member absent, an id that DOES reach the caller's account (the
/// organization root it sits under), one that does not (an OU it is not in), both of them
/// together, and both listing kinds — the paged one and the aggregate summary — for
/// recommended actions and for automation events alike.
#[tokio::test]
async fn the_organizational_unit_restriction_selects_the_accounts_beneath_the_named_nodes() {
    let emulator = Emulator::mounted().await;
    emulator
        .organizations_ok("CreateOrganization", &json!({}), "create")
        .await;
    let root = emulator.root_id("create").await;
    // An OU the caller's account is NOT in: `CreateOrganization` places the management
    // account directly under the root.
    let elsewhere = emulator.create_organizational_unit(&root, "Elsewhere").await;

    // The caller is opted in, so its resources have been analysed: the unrestricted
    // listing is what the restricted ones are compared against.
    let unrestricted = emulator
        .count_rows(
            "ListRecommendedActions",
            "recommendedActions",
            &json!({}),
            "unrestricted",
        )
        .await;
    assert!(
        unrestricted > 0,
        "case \"unrestricted\": the opted-in caller has analysed actions to restrict, got \
         {unrestricted}"
    );

    // One automation event of the caller's account, so the event listings have a row the
    // restriction can select or drop.
    let action = emulator
        .automation_ok(
            "ListRecommendedActions",
            &json!({"maxResults": 1}),
            "one-action",
        )
        .await;
    let action_id = action["recommendedActions"][0]["recommendedActionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    emulator
        .automation_ok(
            "StartAutomationEvent",
            &json!({"recommendedActionId": action_id}),
            "one-event",
        )
        .await;

    // (case, the ids the request names, how many of the caller's rows survive)
    let rows: Vec<(&str, Option<Vec<String>>, usize)> = vec![
        ("no-restriction", None, unrestricted),
        (
            "the-root-the-account-sits-under",
            Some(vec![root.clone()]),
            unrestricted,
        ),
        (
            "an-organizational-unit-the-account-is-not-in",
            Some(vec![elsewhere.clone()]),
            0,
        ),
        (
            "both-nodes-at-once",
            Some(vec![root.clone(), elsewhere.clone()]),
            unrestricted,
        ),
    ];

    for (case, ids, expected) in &rows {
        let mut request = json!({});
        if let Some(ids) = ids {
            request["organizationalUnitIds"] = json!(ids);
        }

        let actions = emulator
            .count_rows(
                "ListRecommendedActions",
                "recommendedActions",
                &request,
                case,
            )
            .await;
        assert_eq!(
            actions, *expected,
            "case {case:?}: ListRecommendedActions reports the actions of the accounts beneath \
             the named nodes, expected {expected}, got {actions}"
        );
        let summarised = emulator
            .summary_count(
                "ListRecommendedActionSummaries",
                "recommendedActionSummaries",
                "recommendedActionCount",
                &request,
                case,
            )
            .await;
        assert_eq!(
            summarised,
            *expected as i64,
            "case {case:?}: the summary covers exactly the actions the listing reports, expected \
             {expected}, got {summarised}"
        );

        let events = emulator
            .count_rows(
                "ListAutomationEvents",
                "automationEvents",
                &request,
                case,
            )
            .await;
        let expected_events = if *expected == 0 { 0 } else { 1 };
        assert_eq!(
            events, expected_events,
            "case {case:?}: ListAutomationEvents reports the events of the accounts beneath the \
             named nodes, expected {expected_events}, got {events}"
        );
        let event_total = emulator
            .summary_count(
                "ListAutomationEventSummaries",
                "automationEventSummaries",
                "automationEventCount",
                &request,
                case,
            )
            .await;
        assert_eq!(
            event_total, expected_events as i64,
            "case {case:?}: the event summary covers exactly the events the listing reports, \
             expected {expected_events}, got {event_total}"
        );
    }
}

/// An organization rule's `organizationConfiguration`, and the `organizationScope` of its
/// preview, are resolved through the same four members: an included OU or organization root
/// must have an effective organization rule mode of `AnyAllowed`, exclusions take precedence
/// over inclusions, and the management account is not reached through an OU or the root.
#[tokio::test]
async fn an_organization_rule_scope_resolves_its_organizational_units_and_exclusions() {
    let emulator = Emulator::mounted().await;
    emulator
        .organizations_ok("CreateOrganization", &json!({}), "create")
        .await;
    let root = emulator.root_id("create").await;
    let unit = emulator.create_organizational_unit(&root, "Workloads").await;
    let caller = emulator.management_account_id("create").await;

    // A rule scoped to an OU whose effective mode is not `AnyAllowed` is refused: "Each
    // included OU or organization root must have an effective organization rule mode of
    // AnyAllowed".
    let rule = |scope: Value| {
        json!({
            "name": "organization-rule",
            "ruleType": "OrganizationRule",
            "recommendedActionTypes": ["UpgradeEbsVolumeType"],
            "schedule": {
                "scheduleExpression": "30 12 * * ? *",
                "scheduleExpressionTimezone": "UTC",
                "executionWindowInMinutes": 60
            },
            "status": "Active",
            "organizationConfiguration": scope,
        })
    };
    let refused = emulator
        .automation("CreateAutomationRule", &rule(json!({"organizationalUnitIds": [unit]})))
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (403, "AccessDeniedException"),
        "case \"an-ou-that-is-not-any-allowed\": expected the modelled 403, got {} with {}",
        refused.status,
        refused.body
    );

    // With the configuration in place the same rule is stored, so the refusal keys on the
    // OU's effective mode and on nothing else.
    emulator.put_on_node(&unit, ANY_ALLOWED, "any-allowed").await;
    emulator
        .automation_ok(
            "CreateAutomationRule",
            &rule(json!({"organizationalUnitIds": [unit]})),
            "an-any-allowed-ou",
        )
        .await;

    // The preview reads the same four members. (case, the scope, how many of the caller's
    // actions it previews)
    let previewable = emulator
        .count_rows(
            "ListRecommendedActions",
            "recommendedActions",
            &json!({}),
            "previewable",
        )
        .await;
    assert!(
        previewable > 0,
        "case \"previewable\": the caller has analysed actions to preview, got {previewable}"
    );
    let rows: Vec<(&str, Value, usize)> = vec![
        ("no-scope-at-all", json!(null), previewable),
        (
            "the-account-named-directly",
            json!({"accountIds": [caller]}),
            previewable,
        ),
        (
            // "The rule doesn't apply to the management account when the account is
            // reached through an OU or the organization root."
            "the-management-account-reached-through-the-root",
            json!({"organizationalUnitIds": [root]}),
            0,
        ),
        (
            // "Exclusions take precedence over inclusions."
            "the-account-named-and-excluded",
            json!({"accountIds": [caller], "excludedAccountIds": [caller]}),
            0,
        ),
        (
            "every-account-less-the-excluded-one",
            json!({"excludedAccountIds": [caller]}),
            0,
        ),
    ];
    for (case, scope, expected) in rows {
        let mut request = json!({
            "ruleType": "OrganizationRule",
            "recommendedActionTypes": ["UpgradeEbsVolumeType", "SnapshotAndDeleteUnattachedEbsVolume"],
        });
        if !scope.is_null() {
            request["organizationScope"] = scope;
        }
        let previewed = emulator
            .count_rows(
                "ListAutomationRulePreview",
                "previewResults",
                &request,
                case,
            )
            .await;
        assert_eq!(
            previewed, expected,
            "case {case:?}: the preview covers the accounts the scope resolves to, expected \
             {expected}, got {previewed}"
        );
        let summarised = emulator
            .summary_count(
                "ListAutomationRulePreviewSummaries",
                "previewResultSummaries",
                "recommendedActionCount",
                &request,
                case,
            )
            .await;
        assert_eq!(
            summarised, expected as i64,
            "case {case:?}: the preview summary covers exactly what the preview reports, \
             expected {expected}, got {summarised}"
        );
    }
}
