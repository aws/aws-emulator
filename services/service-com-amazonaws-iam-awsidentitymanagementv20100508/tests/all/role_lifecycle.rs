// SPDX-License-Identifier: CC0-1.0

//! `CreateRole` and its reporting half `GetRole`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour (status, awsQuery error code, XML members) of a role that was
//! PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share, so a
//! request is built and read exactly one way.
//!
//! The suites are tables: each row differs along what the rule being asserted
//! generalises over (the path, the optional members, the tag count, the case of a
//! name, the form of a trust policy), so no single recorded request is the
//! specification.

use std::collections::BTreeMap;
use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A trust policy with a single statement — the form `CreateRole`'s own modelled
/// example carries.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The same emulator as [`router`], over an engine the caller keeps, so a suite can
/// seed the store directly instead of driving thousands of wire calls to reach a
/// quota. Everything else about the router is identical.
pub(crate) fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// The text of the `<tag>` element the body carries, or `None` when the response
/// omits that member — which is how an optional member is asserted absent.
pub(crate) fn optional(body: &str, tag: &str) -> Option<String> {
    let found = elements(body, tag);
    assert!(
        found.len() <= 1,
        "expected at most one <{tag}> in the response, got {}: {body}",
        found.len()
    );
    found.into_iter().next()
}

/// Borrow an owned parameter list as the harness takes it.
pub(crate) fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

pub(crate) fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The `<Key>`/`<Value>` pairs of a `Tags` list, as a map.
pub(crate) fn reported_tags(body: &str) -> BTreeMap<String, String> {
    elements(body, "Key")
        .into_iter()
        .zip(elements(body, "Value"))
        .collect()
}

/// The trust policy a response reports, and the JSON value it parses to.
///
/// IAM reports a policy document URL-encoded compliant with RFC 3986, so the reported text
/// is percent-decoded first (`crate::user_lifecycle::document_element`, the one decode every
/// suite in this crate reads a document through) — which is what a client that follows the
/// contract does before it parses the policy as JSON.
fn reported_policy(body: &str) -> (String, serde_json::Value) {
    let reported = crate::user_lifecycle::document_element(body, "AssumeRolePolicyDocument");
    let parsed = serde_json::from_str(&reported).unwrap_or_else(|error| {
        panic!("the reported trust policy must be JSON ({error}): {reported:?}")
    });
    (reported, parsed)
}

/// A role ARN is `arn:aws:iam::<account>:role<path><name>` — IAM is global, so the
/// region segment is EMPTY.
fn expected_role_arn(path: &str, role_name: &str) -> String {
    format!("arn:aws:iam::{}:role{path}{role_name}", account_id())
}

/// `Role$RoleId` targets `idType` (length 16..128, pattern `^[\w]+$`) — the modelled
/// constraints on the unique id the service MINTS (never echoes).
fn assert_well_formed_role_id(case: &str, role_id: &str) {
    assert!(
        role_id.len() >= 16 && role_id.len() <= 128,
        "case {case:?}: RoleId must be 16..128 characters, got {role_id:?}"
    );
    assert!(
        role_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "case {case:?}: RoleId must match ^[\\w]+$, got {role_id:?}"
    );
}

/// One `CreateRole` request, with the members the row exercises.
struct CreateCase {
    name: &'static str,
    path: Option<&'static str>,
    /// The path the role must REPORT: the request's, or `/` when it named none.
    expected_path: &'static str,
    description: Option<&'static str>,
    max_session_duration: Option<i32>,
    /// What `MaxSessionDuration` must report: the request's, or the documented
    /// one-hour default.
    expected_max_session_duration: i32,
    permissions_boundary: Option<String>,
    tags: Vec<(String, String)>,
    document: String,
}

impl CreateCase {
    fn params(&self, role_name: &str) -> Vec<(String, String)> {
        let mut params = vec![
            param("RoleName", role_name),
            param("AssumeRolePolicyDocument", &self.document),
        ];
        if let Some(path) = self.path {
            params.push(param("Path", path));
        }
        if let Some(description) = self.description {
            params.push(param("Description", description));
        }
        if let Some(duration) = self.max_session_duration {
            params.push(param("MaxSessionDuration", &duration.to_string()));
        }
        if let Some(boundary) = &self.permissions_boundary {
            params.push(param("PermissionsBoundary", boundary));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }
}

/// A policy ARN satisfying `arnType` (length 20..2048), used as a permissions
/// boundary.
fn managed_policy_arn() -> String {
    format!("arn:aws:iam::{}:policy/Boundary", account_id())
}

/// `CreateRole` answers from the record it persisted, and `GetRole` reads that same
/// record back: one row per thing the rule generalises over — the path (absent, root,
/// nested, deeply nested), the optional members, `MaxSessionDuration` at each
/// published bound of `roleMaxSessionDurationType` (3600..43200), the tag count
/// (none, one, two, the `tagListType` bound of 50) and the form of the trust policy.
#[tokio::test]
async fn create_persists_every_member_and_get_role_reads_it_back() {
    let fifty_tags: Vec<(String, String)> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();

    let cases = [
        CreateCase {
            name: "no optional member at all",
            path: None,
            expected_path: "/",
            description: None,
            max_session_duration: None,
            expected_max_session_duration:
                service_com_amazonaws_iam_awsidentitymanagementv20100508::state::DEFAULT_MAX_SESSION_DURATION_SECONDS,
            permissions_boundary: None,
            tags: Vec::new(),
            document: TRUST_POLICY.to_string(),
        },
        CreateCase {
            name: "explicit root path and one tag",
            path: Some("/"),
            expected_path: "/",
            description: Some("the DLM execution role"),
            max_session_duration: None,
            expected_max_session_duration:
                service_com_amazonaws_iam_awsidentitymanagementv20100508::state::DEFAULT_MAX_SESSION_DURATION_SECONDS,
            permissions_boundary: None,
            tags: vec![("team".to_string(), "storage".to_string())],
            document: TRUST_POLICY.to_string(),
        },
        CreateCase {
            name: "nested path, session duration at the published minimum",
            path: Some("/service-role/"),
            expected_path: "/service-role/",
            description: Some(""),
            max_session_duration: Some(3_600),
            expected_max_session_duration: 3_600,
            permissions_boundary: None,
            tags: Vec::new(),
            document: TRUST_POLICY.to_string(),
        },
        CreateCase {
            name: "deeply nested path, session duration at the published maximum",
            path: Some("/application/eu/team-1/"),
            expected_path: "/application/eu/team-1/",
            description: Some("twelve hour sessions"),
            max_session_duration: Some(43_200),
            expected_max_session_duration: 43_200,
            permissions_boundary: Some(managed_policy_arn()),
            tags: vec![
                ("owner".to_string(), "platform".to_string()),
                ("cost:center".to_string(), "1234".to_string()),
            ],
            document: TRUST_POLICY.to_string(),
        },
        CreateCase {
            name: "a single statement object, the tagListType bound of 50 tags",
            path: None,
            expected_path: "/",
            description: None,
            max_session_duration: Some(7_200),
            expected_max_session_duration: 7_200,
            permissions_boundary: None,
            tags: fifty_tags,
            document: r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"sts:AssumeRole"}}"#
                .to_string(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let role_name = format!("role-{index}");
        let owned = case.params(&role_name);
        let (status, body) = call(&router, "CreateRole", &borrowed(&owned)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateRole to answer 200, got {status}: {body}",
            case.name
        );

        assert_well_formed_role_id(case.name, &element(&body, "RoleId"));
        assert_eq!(
            element(&body, "RoleName"),
            role_name,
            "case {:?}: expected RoleName {role_name:?}, got {body}",
            case.name
        );
        assert_eq!(
            element(&body, "Path"),
            case.expected_path,
            "case {:?}: expected Path {:?}, got {body}",
            case.name,
            case.expected_path
        );
        assert_eq!(
            element(&body, "Arn"),
            expected_role_arn(case.expected_path, &role_name),
            "case {:?}: expected the global IAM role ARN, got {body}",
            case.name
        );
        assert_eq!(
            element(&body, "MaxSessionDuration"),
            case.expected_max_session_duration.to_string(),
            "case {:?}: expected MaxSessionDuration {}, got {body}",
            case.name,
            case.expected_max_session_duration
        );
        assert_eq!(
            optional(&body, "Description").as_deref(),
            case.description,
            "case {:?}: expected Description {:?}, got {body}",
            case.name,
            case.description
        );
        let (reported_document, parsed_document) = reported_policy(&body);
        assert_eq!(
            reported_document, case.document,
            "case {:?}: expected the trust policy the request carried, got {body}",
            case.name
        );
        assert_eq!(
            parsed_document,
            serde_json::from_str::<serde_json::Value>(&case.document).expect("a JSON policy"),
            "case {:?}: the reported trust policy must parse to the policy that was \
             sent, got {body}",
            case.name
        );
        assert_eq!(
            reported_tags(&body),
            case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
            "case {:?}: expected the tags the request carried, got {body}",
            case.name
        );
        assert_eq!(
            optional(&body, "PermissionsBoundaryArn"),
            case.permissions_boundary,
            "case {:?}: expected PermissionsBoundaryArn {:?}, got {body}",
            case.name,
            case.permissions_boundary
        );
        assert_eq!(
            optional(&body, "PermissionsBoundaryType"),
            case.permissions_boundary.as_ref().map(|_| {
                service_com_amazonaws_iam_awsidentitymanagementv20100508::state::PERMISSIONS_BOUNDARY_TYPE
                    .to_string()
            }),
            "case {:?}: expected the one modelled PermissionsBoundaryType exactly when \
             a boundary is attached, got {body}",
            case.name
        );
        assert!(
            !element(&body, "CreateDate").is_empty(),
            "case {:?}: CreateDate is required, got {body}",
            case.name
        );

        // The round trip: every member the create reported is read back off the
        // stored record, unchanged.
        let (get_status, got) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: expected GetRole to answer 200, got {get_status}: {got}",
            case.name
        );
        for member in [
            "RoleName",
            "RoleId",
            "Path",
            "Arn",
            "Description",
            "MaxSessionDuration",
            "AssumeRolePolicyDocument",
            "CreateDate",
            "PermissionsBoundaryArn",
        ] {
            assert_eq!(
                optional(&got, member),
                optional(&body, member),
                "case {:?}: GetRole must report the same {member} as the create, got {got}",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&got),
            reported_tags(&body),
            "case {:?}: GetRole must report the same tags as the create, got {got}",
            case.name
        );
    }
}

/// Role names are unique within the account and NOT distinguished by case
/// (`CreateRole$RoleName`), so a second create under any casing of a held name is
/// refused — and leaves the first role untouched.
#[tokio::test]
async fn a_held_role_name_is_refused_whatever_its_case() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "MyRole"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "the first create must succeed: {created}");
    let original_id = element(&created, "RoleId");

    for duplicate in ["MyRole", "myrole", "MYROLE", "mYrOlE"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", duplicate),
                // A different trust policy and path: it is the NAME that conflicts.
                (
                    "AssumeRolePolicyDocument",
                    r#"{"Statement":[{"Effect":"Deny","Action":"*","Principal":{"AWS":"*"}}]}"#,
                ),
                ("Path", "/other/"),
            ],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {duplicate:?}: expected 409 for a held name, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {duplicate:?}: expected the modelled awsQuery code, got {body}"
        );

        // The held role is unchanged: same id, same path.
        let (_, got) = call(&router, "GetRole", &[("RoleName", duplicate)]).await;
        assert_eq!(
            element(&got, "RoleId"),
            original_id,
            "case {duplicate:?}: the refused create must not replace the held role: {got}"
        );
        assert_eq!(
            element(&got, "Path"),
            "/",
            "case {duplicate:?}: the refused create must not change the held path: {got}"
        );
    }
}

/// A trust policy must be an IAM policy document — a JSON object carrying a
/// `Statement` element, the rule `crate::state::is_policy_document` holds for every
/// operation in this crate that takes one — and anything else is refused with the
/// modelled `MalformedPolicyDocumentException`, whatever `policyDocumentType`'s own
/// pattern allows. One row per form the rule generalises over.
#[tokio::test]
async fn a_trust_policy_that_is_not_a_json_policy_is_refused() {
    let refused = [
        ("not JSON at all", "this is not a policy"),
        ("a JSON string", r#""Statement""#),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        (
            "an object carrying every element but Statement",
            r#"{"Version":"2012-10-17","Id":"one"}"#,
        ),
        ("an empty object", "{}"),
        ("a truncated object", r#"{"Statement":[{"Effect":"Allow"}"#),
        // A role's trust policy is a RESOURCE-based policy, so every statement of it names
        // the principal the role trusts; one that names none trusts nobody.
        (
            "a statement naming no principal",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:AssumeRole"}]}"#,
        ),
    ];

    let router = router();
    for (index, (case, document)) in refused.iter().enumerate() {
        let role_name = format!("refused-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a malformed trust policy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        // Nothing was persisted: the role the refused create named does not exist.
        let (get_status, got) =
            call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused create must persist nothing, got {get_status}: {got}"
        );
    }

    let accepted = [
        (
            "a statement object",
            r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"sts:AssumeRole","Principal":{"Service":"dlm.amazonaws.com"}}}"#,
        ),
        (
            "a list of two statements",
            r#"{"Statement":[{"Effect":"Allow","Action":"sts:AssumeRole","Principal":{"Service":"dlm.amazonaws.com"}},{"Effect":"Deny","Action":"sts:AssumeRole","Principal":{"AWS":"*"}}]}"#,
        ),
        (
            "a statement list with no Version",
            r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"*"}}]}"#,
        ),
        // The per-statement rule a TRUST policy is held to is the principal it names
        // (`state::trust_policy_document_defect`); nothing below that is inspected, so a
        // document whose statements carry no action is STORED, not refused.
        (
            "the smallest document the grammar admits",
            r#"{"Statement":[]}"#,
        ),
        (
            "a Statement that is not a statement list",
            r#"{"Statement":"Allow"}"#,
        ),
    ];
    for (index, (case, document)) in accepted.iter().enumerate() {
        let role_name = format!("accepted-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a JSON policy to be accepted, got {status}: {body}"
        );
        assert_eq!(
            reported_policy(&body).0,
            *document,
            "case {case:?}: the stored document must read back as it was sent, got {body}"
        );
    }
}

/// AWS reserves the `aws:` tag-key prefix, so a create naming one is refused with the
/// modelled `InvalidInputException` and persists nothing; a key that merely CONTAINS
/// those characters is fine.
#[tokio::test]
async fn a_reserved_tag_key_is_refused() {
    let refused = [
        ("lower case prefix", "aws:cost-center"),
        ("upper case prefix", "AWS:CostCenter"),
        ("mixed case prefix", "Aws:team"),
        ("the bare prefix", "aws:"),
    ];

    let router = router();
    for (index, (case, key)) in refused.iter().enumerate() {
        let role_name = format!("reserved-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("Tags.member.1.Key", "team"),
                ("Tags.member.1.Value", "storage"),
                ("Tags.member.2.Key", key),
                ("Tags.member.2.Value", "x"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for the reserved key {key:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        let (get_status, got) =
            call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused create must persist nothing, got {get_status}: {got}"
        );
    }

    let accepted = [
        ("a key containing aws", "awsome"),
        ("a key ending in aws:", "team-aws:x"),
        ("a key whose VALUE starts with aws:", "team"),
    ];
    for (index, (case, key)) in accepted.iter().enumerate() {
        let role_name = format!("unreserved-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("Tags.member.1.Key", key),
                ("Tags.member.1.Value", "aws:value"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the key {key:?} to be accepted, got {status}: {body}"
        );
        assert_eq!(
            reported_tags(&body).get(*key).map(String::as_str),
            Some("aws:value"),
            "case {case:?}: expected the tag to be stored, got {body}"
        );
    }
}

/// `GetRole` on a name the account does not hold is the modelled
/// `NoSuchEntityException` (HTTP 404), whatever the name looks like — and a read
/// changes nothing a later caller sees.
#[tokio::test]
async fn get_role_on_a_name_the_account_does_not_hold_is_not_found() {
    let router = router();
    for case in ["absent", "Absent", "a", "role_with+every=allowed.char@-"] {
        let (status, body) = call(&router, "GetRole", &[("RoleName", case)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an unheld role, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The failed reads minted nothing: the create that follows still succeeds, and
    // the role reads back afterwards.
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "absent"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "create after the failed reads: {body}");
    let (status, got) = call(&router, "GetRole", &[("RoleName", "ABSENT")]).await;
    assert_eq!(status, 200, "the created role must read back: {got}");
    assert_eq!(
        element(&got, "RoleId"),
        element(&body, "RoleId"),
        "a read must report the id the create minted: {got}"
    );
}

/// Each role gets its own stable unique id: distinct across roles, and the same on
/// every read of one role.
#[tokio::test]
async fn minted_role_ids_are_unique_and_stable() {
    let router = router();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..8 {
        let role_name = format!("unique-{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {role_name:?}: create failed: {body}");
        let role_id = element(&body, "RoleId");
        assert_well_formed_role_id(&role_name, &role_id);

        // Stable: two reads of the same role report the same id.
        for read in 0..2 {
            let (status, got) =
                call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
            assert_eq!(status, 200, "case {role_name:?}: read {read} failed: {got}");
            assert_eq!(
                element(&got, "RoleId"),
                role_id,
                "case {role_name:?}: read {read} must report the same RoleId: {got}"
            );
        }
        ids.push(role_id);
    }

    let unique: BTreeMap<&String, ()> = ids.iter().map(|id| (id, ())).collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every role must get its own RoleId, got {ids:?}"
    );
}

/// `MaxSessionDuration` outside the modelled range of `roleMaxSessionDurationType`
/// (3600..43200) is refused before the handler runs, and nothing is persisted. Each
/// published bound is accepted by the first suite; this row set is one step past each
/// of them.
#[tokio::test]
async fn a_session_duration_outside_the_modelled_range_is_refused() {
    let router = router();
    for (case, duration) in [
        ("one second below the published minimum", "3599"),
        ("one second above the published maximum", "43201"),
        ("zero", "0"),
    ] {
        let role_name = format!("duration-{duration}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("MaxSessionDuration", duration),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for MaxSessionDuration {duration}, got {status}: {body}"
        );
        let (get_status, got) =
            call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused create must persist nothing, got {get_status}: {got}"
        );
    }
}

/// One already-held role, as the store holds it. Every member is filled the way
/// `CreateRole` fills it, so a seeded role is indistinguishable from a created one.
fn seeded_role(
    role_name: &str,
) -> service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
    use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
    service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
        path: "/".to_string(),
        role_name: role_name.to_string(),
        role_id: format!("AROASEEDED{role_name}").replace('-', "_"),
        arn: expected_role_arn("/", role_name),
        create_date: state::now(),
        assume_role_policy_document: Some(TRUST_POLICY.to_string()),
        description: None,
        max_session_duration: Some(state::DEFAULT_MAX_SESSION_DURATION_SECONDS),
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        source_role_template: None,
    }
}

/// An account holds a bounded number of roles: at the quota a create is refused with
/// the modelled `LimitExceededException`, and one short of it still succeeds. The
/// store is seeded directly, so the bound is exercised without a thousand wire calls.
#[tokio::test]
async fn the_account_role_quota_refuses_one_role_too_many() {
    use service_com_amazonaws_iam_awsidentitymanagementv20100508::{
        state::MAX_ROLES_PER_ACCOUNT, ComAmazonawsIamAwsidentitymanagementv20100508State,
    };

    for (case, seeded, expected_status) in [
        ("one short of the quota", MAX_ROLES_PER_ACCOUNT - 1, 200),
        ("at the quota", MAX_ROLES_PER_ACCOUNT, 409),
        ("past the quota", MAX_ROLES_PER_ACCOUNT + 1, 409),
    ] {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        {
            let state = engine.state::<ComAmazonawsIamAwsidentitymanagementv20100508State>();
            let mut store = state.lock();
            for index in 0..seeded {
                let role_name = format!("seeded-{index}");
                store
                    .roles
                    .insert(role_name.to_ascii_lowercase(), seeded_role(&role_name));
            }
        }
        let router = router_on(engine);

        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "one-more-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: with {seeded} roles held, expected HTTP {expected_status}, \
             got {status}: {body}"
        );
        if expected_status == 200 {
            assert_well_formed_role_id(case, &element(&body, "RoleId"));
        } else {
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            // Nothing was added past the bound.
            let (get_status, got) =
                call(&router, "GetRole", &[("RoleName", "one-more-role")]).await;
            assert_eq!(
                get_status, 404,
                "case {case:?}: the refused create must persist nothing, got {got}"
            );
        }
    }
}
