// SPDX-License-Identifier: CC0-1.0

//! How this service BUILDS the message of a `ValidationException`, over the wire.
//!
//! THE RULE: a request refused for breaking an input constraint answers HTTP 400
//! `ValidationException` whose message is built from the REQUEST — the AWS envelope
//! `N validation error(s) detected: ` over one clause per offending member, each naming
//! that member's path and the constraint it violated:
//!
//! * a value outside a `@length`, `@pattern`, `@range` or `@uniqueItems` constraint is
//!   reported WITHOUT the value — `Value at '<member>' failed to satisfy constraint: …`;
//! * a value outside an ENUM's value set IS quoted — `Value '<value>' at '<member>' …`;
//! * an enum requirement reads `Member must be one of: [ … ]` over the type's values, in
//!   the order this service advertises that value set in (which for `ProviderType` is not
//!   the model's declaration order);
//! * an absent `@required` member is reported as the `null` it was;
//! * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors the
//!   model spells it with, so a caller can re-apply exactly what it was advertised;
//! * a member that names a CONNECTION by ARN is held, on the repository-link surface, to
//!   the connection-scoped expression this service advertises rather than to the generic
//!   ARN pattern the public model carries.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over — the constraint class, the member, the length/size of the rejected
//! value (including values far past the framework's own echo cap), the ARN form, and the
//! enum. Every constraint quoted below comes from `smithy_query(query="operation",
//! operation="<Op>")` / `smithy_query(query="shape", shape="<Shape>")` on this service's
//! model; the five cases the issue cited are five of these rows.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a signed
//! SDK request takes.

use super::connection_lifecycle::{REGION, call, create, router};

/// `HostArn`'s `@pattern`, as a clause must quote it: the model's expression
/// (`^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$`)
/// without its anchors.
const HOST_ARN_EXPRESSION: &str =
    r"arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+";

/// `KmsKeyArn`'s `@pattern`, unanchored
/// (`^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`).
const KMS_KEY_ARN_EXPRESSION: &str = r"arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+";

/// `IamRoleArn`'s `@pattern`, unanchored
/// (`^arn:aws(-[\w]+)*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$`).
const IAM_ROLE_ARN_EXPRESSION: &str = r"arn:aws(-[\w]+)*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+";

/// `RepositoryLinkId`'s `@pattern`, unanchored.
const REPOSITORY_LINK_ID_EXPRESSION: &str =
    r"[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}";

/// The expression this service advertises — and enforces — for a connection ARN, written
/// out as the LITERAL the recorded service returns rather than read off the
/// implementation's own constant, so rewording that constant fails this suite.
const CONNECTION_ARN_EXPRESSION: &str = r"arn:aws(-[\w]+)*:(codestar-connections|codeconnections):[a-z0-9\-]+:[0-9]{12}:connection\/[\w\-]+";

/// The `ProviderType` value set as this service ADVERTISES it, written out as the literal
/// the recorded service returns.
///
/// The bracketed sequence is fixed text of the refusal: the service states these members in
/// this order, which is not the order the model declares them in
/// (`smithy_query(query="shape", shape="ProviderType")` gives `Bitbucket, GitHub,
/// GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps,
/// GitHubEnterpriseCloudDataResidency`), so the order is pinned here rather than derived
/// from the enum. The membership is the model's: the value the shape gained after those
/// refusals were recorded (`GitHubEnterpriseCloudDataResidency`) is stated last.
const PROVIDER_TYPES: &str = "[GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, \
     GitLabSelfManaged, Bitbucket, GitHubEnterpriseCloudDataResidency]";

/// The `SyncConfigurationType` value set as the model declares it: one value.
const SYNC_TYPES: &str = "[CFN_STACK_SYNC]";

/// The envelope one refused member states.
fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// A clause that does NOT quote the rejected value: every constraint class but an enum's
/// value set.
fn unquoted(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// A clause that quotes the rejected value: an enum's value set.
fn quoted(value: &str, member: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` violation states.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The requirement an enum violation states.
fn enum_requirement(value_set: &str) -> String {
    format!("Member must be one of: {value_set}")
}

/// The message a refused response carries, under either spelling of the member.
fn refusal_message(case: &str, body: &serde_json::Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message, got {body}"))
        .to_string()
}

/// Assert `body` is a 400 `ValidationException` stating exactly `expected`.
fn assert_validation_message(
    case: &str,
    expected: &str,
    status: u16,
    body: &serde_json::Value,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .is_some_and(|kind| kind.ends_with("ValidationException")),
        "case {case:?}: expected ValidationException, got {body}"
    );
    let actual = refusal_message(case, body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

/// One request that breaks one modelled constraint, and the message it must answer.
struct MessageCase {
    label: String,
    operation: &'static str,
    body: serde_json::Value,
    message: String,
}

/// An ARN this service's `HostArn` pattern admits, for a host nothing holds: used only
/// where a row is about the ARN's SYNTAX.
fn host_arn(resource: &str) -> String {
    format!(
        "arn:aws:codeconnections:{REGION}:{}:host/{resource}",
        engine::emulator_account_id()
    )
}

/// The `@length` rows: a string past its maximum, and one below its minimum, over four
/// members with four different bounds and over rejected values from 0 bytes to far past
/// the framework's own 256-byte echo cap.
fn length_cases() -> Vec<MessageCase> {
    let mut cases = Vec::new();

    // `CreateConnection.ConnectionName` is `length 1..32`.
    for length in [33_usize, 40, 257, 2048] {
        cases.push(MessageCase {
            label: format!("ConnectionName of {length} characters (max 32)"),
            operation: "CreateConnection",
            body: serde_json::json!({
                "ConnectionName": "n".repeat(length),
                "ProviderType": "GitHub",
            }),
            message: one_error(&unquoted(
                "connectionName",
                "Member must have length less than or equal to 32",
            )),
        });
    }
    cases.push(MessageCase {
        label: "ConnectionName of 0 characters (min 1)".to_string(),
        operation: "CreateConnection",
        body: serde_json::json!({ "ConnectionName": "", "ProviderType": "GitHub" }),
        message: one_error(&unquoted(
            "connectionName",
            "Member must have length greater than or equal to 1",
        )),
    });

    // `CreateHost.ProviderEndpoint` is the modelled `Url`, `length 1..512`.
    for length in [513_usize, 620, 4096] {
        cases.push(MessageCase {
            label: format!("ProviderEndpoint of {length} characters (max 512)"),
            operation: "CreateHost",
            body: serde_json::json!({
                "Name": "demo-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": format!("https://{}.example.com", "a".repeat(length - 20)),
            }),
            message: one_error(&unquoted(
                "providerEndpoint",
                "Member must have length less than or equal to 512",
            )),
        });
    }

    // `CreateHost.Name` is `length 1..64`.
    cases.push(MessageCase {
        label: "CreateHost Name of 65 characters (max 64)".to_string(),
        operation: "CreateHost",
        body: serde_json::json!({
            "Name": "n".repeat(65),
            "ProviderType": "GitHubEnterpriseServer",
            "ProviderEndpoint": "https://ghe.example.internal",
        }),
        message: one_error(&unquoted(
            "name",
            "Member must have length less than or equal to 64",
        )),
    });

    // `UpdateSyncBlocker.ResolvedReason` is `length 1..250`.
    cases.push(MessageCase {
        label: "UpdateSyncBlocker ResolvedReason of 251 characters (max 250)".to_string(),
        operation: "UpdateSyncBlocker",
        body: serde_json::json!({
            "Id": "blocker-1",
            "SyncType": "CFN_STACK_SYNC",
            "ResourceName": "DemoStack",
            "ResolvedReason": "r".repeat(251),
        }),
        message: one_error(&unquoted(
            "resolvedReason",
            "Member must have length less than or equal to 250",
        )),
    });

    cases
}

/// The `@pattern` rows: five members carrying five different expressions, each refused
/// with its own expression quoted unanchored and the value left out.
fn pattern_cases() -> Vec<MessageCase> {
    let account = engine::emulator_account_id();
    let mut cases = Vec::new();

    for (label, value) in [
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "another service's namespace",
            format!("arn:aws:codebuild:{REGION}:{account}:host/abc"),
        ),
        (
            "an account id that is not 12 digits",
            format!("arn:aws:codeconnections:{REGION}:1234:host/abc"),
        ),
        ("no resource after host/", host_arn("")),
    ] {
        cases.push(MessageCase {
            label: format!("CreateConnection HostArn: {label}"),
            operation: "CreateConnection",
            body: serde_json::json!({
                "ConnectionName": "demo-conn",
                "ProviderType": "GitHub",
                "HostArn": value,
            }),
            message: one_error(&unquoted(
                "hostArn",
                &pattern_requirement(HOST_ARN_EXPRESSION),
            )),
        });
    }

    // `ListConnections.HostArnFilter` targets the SAME shape, so the same expression is
    // advertised for a different member of a different operation.
    cases.push(MessageCase {
        label: "ListConnections HostArnFilter: not an ARN at all".to_string(),
        operation: "ListConnections",
        body: serde_json::json!({ "HostArnFilter": "not-an-arn" }),
        message: one_error(&unquoted(
            "hostArnFilter",
            &pattern_requirement(HOST_ARN_EXPRESSION),
        )),
    });

    // `GetRepositoryLink.RepositoryLinkId` is UUID-shaped.
    for (label, value) in [
        ("not a uuid", "not-a-uuid".to_string()),
        ("a uuid without its hyphens", "0f1e2d3c4b5a69788796a5b4c3d2e1f0".to_string()),
        ("36 non-hex characters", "g".repeat(36)),
    ] {
        cases.push(MessageCase {
            label: format!("GetRepositoryLink RepositoryLinkId: {label}"),
            operation: "GetRepositoryLink",
            body: serde_json::json!({ "RepositoryLinkId": value }),
            message: one_error(&unquoted(
                "repositoryLinkId",
                &pattern_requirement(REPOSITORY_LINK_ID_EXPRESSION),
            )),
        });
    }

    // `CreateSyncConfiguration.RoleArn` is an IAM role ARN.
    cases.push(MessageCase {
        label: "CreateSyncConfiguration RoleArn: not a role ARN".to_string(),
        operation: "CreateSyncConfiguration",
        body: serde_json::json!({
            "Branch": "main",
            "ConfigFile": "sync.yaml",
            "RepositoryLinkId": "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            "ResourceName": "DemoStack",
            "RoleArn": "not-a-role",
            "SyncType": "CFN_STACK_SYNC",
        }),
        message: one_error(&unquoted(
            "roleArn",
            &pattern_requirement(IAM_ROLE_ARN_EXPRESSION),
        )),
    });

    // `UpdateRepositoryLink.EncryptionKeyArn` is a KMS key ARN.
    cases.push(MessageCase {
        label: "UpdateRepositoryLink EncryptionKeyArn: not a key ARN".to_string(),
        operation: "UpdateRepositoryLink",
        body: serde_json::json!({
            "RepositoryLinkId": "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            "EncryptionKeyArn": "not-a-key-arn",
        }),
        message: one_error(&unquoted(
            "encryptionKeyArn",
            &pattern_requirement(KMS_KEY_ARN_EXPRESSION),
        )),
    });

    cases
}

/// The enum rows: a value outside a value set, over both of this service's enums and
/// over three members of them. These are the only rows whose clause QUOTES the value.
fn enum_cases() -> Vec<MessageCase> {
    let mut cases = Vec::new();

    for value in ["NotARealProvider", "github", "GITHUB", "GitHub "] {
        cases.push(MessageCase {
            label: format!("CreateConnection ProviderType {value:?}"),
            operation: "CreateConnection",
            body: serde_json::json!({ "ConnectionName": "demo-conn", "ProviderType": value }),
            message: one_error(&quoted(
                value,
                "providerType",
                &enum_requirement(PROVIDER_TYPES),
            )),
        });
    }
    cases.push(MessageCase {
        label: "CreateHost ProviderType 'NotARealProvider'".to_string(),
        operation: "CreateHost",
        body: serde_json::json!({
            "Name": "demo-host",
            "ProviderType": "NotARealProvider",
            "ProviderEndpoint": "https://ghe.example.internal",
        }),
        message: one_error(&quoted(
            "NotARealProvider",
            "providerType",
            &enum_requirement(PROVIDER_TYPES),
        )),
    });
    cases.push(MessageCase {
        label: "ListConnections ProviderTypeFilter 'Nope'".to_string(),
        operation: "ListConnections",
        body: serde_json::json!({ "ProviderTypeFilter": "Nope" }),
        message: one_error(&quoted(
            "Nope",
            "providerTypeFilter",
            &enum_requirement(PROVIDER_TYPES),
        )),
    });
    cases.push(MessageCase {
        label: "GetSyncConfiguration SyncType 'NOT_A_SYNC_TYPE'".to_string(),
        operation: "GetSyncConfiguration",
        body: serde_json::json!({
            "SyncType": "NOT_A_SYNC_TYPE",
            "ResourceName": "DemoStack",
        }),
        message: one_error(&quoted(
            "NOT_A_SYNC_TYPE",
            "syncType",
            &enum_requirement(SYNC_TYPES),
        )),
    });

    cases
}

/// The `@range` rows: `MaxResults` is `range 0..100` on both paginated readers, so a
/// request outside it is refused naming the bound and not the value.
fn range_cases() -> Vec<MessageCase> {
    let mut cases = Vec::new();
    for (operation, value, requirement) in [
        (
            "ListConnections",
            101,
            "Member must have value less than or equal to 100",
        ),
        (
            "ListConnections",
            1000,
            "Member must have value less than or equal to 100",
        ),
        (
            "ListConnections",
            -1,
            "Member must have value greater than or equal to 0",
        ),
        (
            "ListRepositoryLinks",
            101,
            "Member must have value less than or equal to 100",
        ),
    ] {
        cases.push(MessageCase {
            label: format!("{operation} MaxResults {value}"),
            operation,
            body: serde_json::json!({ "MaxResults": value }),
            message: one_error(&unquoted("maxResults", requirement)),
        });
    }
    cases
}

/// The absent-member rows: a `@required` member the request omits is reported as the
/// `null` it was, over three operations.
fn absent_member_cases() -> Vec<MessageCase> {
    vec![
        MessageCase {
            label: "CreateConnection without ConnectionName".to_string(),
            operation: "CreateConnection",
            body: serde_json::json!({ "ProviderType": "GitHub" }),
            message: one_error(
                "Value null at 'connectionName' failed to satisfy constraint: Member must not be \
                 null",
            ),
        },
        MessageCase {
            label: "GetConnection without ConnectionArn".to_string(),
            operation: "GetConnection",
            body: serde_json::json!({}),
            message: one_error(
                "Value null at 'connectionArn' failed to satisfy constraint: Member must not be \
                 null",
            ),
        },
        MessageCase {
            label: "GetRepositoryLink without RepositoryLinkId".to_string(),
            operation: "GetRepositoryLink",
            body: serde_json::json!({}),
            message: one_error(
                "Value null at 'repositoryLinkId' failed to satisfy constraint: Member must not \
                 be null",
            ),
        },
    ]
}

/// RULE: every refused row answers 400 `ValidationException` whose message is exactly the
/// clause its constraint class states — the value quoted only for an enum's value set, a
/// `@pattern` quoted unanchored, an absent member reported as `null`.
#[tokio::test]
async fn a_refusal_states_the_member_and_the_constraint_it_violated() {
    let router = router();

    let cases: Vec<MessageCase> = length_cases()
        .into_iter()
        .chain(pattern_cases())
        .chain(enum_cases())
        .chain(range_cases())
        .chain(absent_member_cases())
        .collect();
    assert!(
        cases.len() > 20,
        "expected the table to span the constraint classes, got {} rows",
        cases.len()
    );

    for case in &cases {
        let (status, body) = call(&router, case.operation, case.body.clone()).await;
        assert_validation_message(&case.label, &case.message, status, &body);
    }

    // Nothing above persisted anything.
    let (status, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "ListConnections: expected 200, got {status} {listed}"
    );
    assert_eq!(
        listed["Connections"],
        serde_json::json!([]),
        "a refused request must persist nothing, got {listed}"
    );
}

/// RULE, stated as what the message must NOT carry: the rejected value never reaches a
/// `@length` or `@pattern` clause, so neither the caller's bytes nor the framework's
/// `… [N bytes]` elision marker can appear however large the value was.
#[tokio::test]
async fn a_length_or_pattern_refusal_echoes_nothing_of_the_rejected_value() {
    let router = router();

    // Rows differ in the size of the value (either side of the framework's 256-byte echo
    // cap) and in the constraint class, and each value carries a marker string that must
    // not come back.
    let rows: Vec<(&str, &'static str, serde_json::Value, String)> = vec![
        (
            "a 40-character ConnectionName",
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": format!("sim2real-conn-{}", "f".repeat(26)),
                "ProviderType": "GitHub",
            }),
            "f".repeat(26),
        ),
        (
            "a 620-byte ProviderEndpoint",
            "CreateHost",
            serde_json::json!({
                "Name": "demo-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": format!("https://{}.example.com", "a".repeat(600)),
            }),
            "a".repeat(600),
        ),
        (
            "a malformed HostArn",
            "CreateConnection",
            serde_json::json!({
                "ConnectionName": "ok",
                "ProviderType": "GitHub",
                "HostArn": "not-an-arn-at-all",
            }),
            "not-an-arn-at-all".to_string(),
        ),
    ];

    for (label, operation, body, needle) in rows {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {answered}"
        );
        let message = refusal_message(label, &answered);
        assert!(
            !message.contains(&needle),
            "case {label:?}: the rejected value must not be echoed, got {message:?}"
        );
        assert!(
            !message.contains("bytes]"),
            "case {label:?}: the elision marker must not be echoed, got {message:?}"
        );
        assert!(
            !message.contains("Value '"),
            "case {label:?}: a constraint clause quotes no value, got {message:?}"
        );
    }
}

/// RULE: more than one offending member states more than one clause, in the counted
/// envelope the framework words plural.
#[tokio::test]
async fn several_offending_members_state_one_clause_each() {
    let router = router();

    let (status, body) = call(
        &router,
        "CreateHost",
        serde_json::json!({
            "Name": "n".repeat(65),
            "ProviderType": "NotARealProvider",
            "ProviderEndpoint": "e".repeat(513),
        }),
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status} {body}");
    let message = refusal_message("three offending members", &body);
    assert!(
        message.starts_with("3 validation errors detected: "),
        "expected the counted plural envelope, got {message:?}"
    );
    for clause in [
        unquoted("name", "Member must have length less than or equal to 64"),
        unquoted(
            "providerEndpoint",
            "Member must have length less than or equal to 512",
        ),
        quoted(
            "NotARealProvider",
            "providerType",
            &enum_requirement(PROVIDER_TYPES),
        ),
    ] {
        assert!(
            message.contains(&clause),
            "expected the clause {clause:?}, got {message:?}"
        );
    }
    assert_eq!(
        message.matches("; ").count(),
        2,
        "expected three clauses joined by '; ', got {message:?}"
    );
}

/// RULE: on the repository-link surface a member that names a connection by ARN is held
/// to the connection-scoped expression — this service's namespace, a region, a 12-digit
/// account and a `connection/<id>` resource — and the refusal advertises THAT expression,
/// not the generic ARN pattern the public model carries.
///
/// Rows differ in HOW the ARN misses the expression: not an ARN at all, an ARN of another
/// resource kind of this service, an ARN of another service, a malformed account, an empty
/// resource id. The first and the fourth also miss the model's own loose pattern, so they
/// are refused at the boundary while the rest reach the handler — and the message is the
/// same either way, which is what makes it one rule rather than two.
#[tokio::test]
async fn a_connection_arn_is_held_to_the_connection_scoped_expression() {
    let router = router();
    let account = engine::emulator_account_id();
    let connection = create(&router, "linked", "GitHub").await;

    // A link to hold the `UpdateRepositoryLink` rows, created from the live connection.
    let (status, created) = call(
        &router,
        "CreateRepositoryLink",
        serde_json::json!({
            "ConnectionArn": connection,
            "OwnerId": "an-owner",
            "RepositoryName": "a-repo",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a link over a well-formed connection ARN must be created, got {status} {created}"
    );
    let link = created["RepositoryLinkInfo"].clone();
    let link_id = link["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no RepositoryLinkId: {created}"))
        .to_string();

    let expected = one_error(&unquoted(
        "connectionArn",
        &pattern_requirement(CONNECTION_ARN_EXPRESSION),
    ));
    let refused = [
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "a host ARN of this service",
            host_arn("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
        ),
        (
            "a repository-link ARN of this service",
            format!("arn:aws:codeconnections:{REGION}:{account}:repository-link/{link_id}"),
        ),
        (
            "another service's connection-shaped ARN",
            format!("arn:aws:codebuild:{REGION}:{account}:connection/c1"),
        ),
        (
            "an account id that is not 12 digits",
            format!("arn:aws:codeconnections:{REGION}:1234:connection/c1"),
        ),
        (
            "no resource after connection/",
            format!("arn:aws:codeconnections:{REGION}:{account}:connection/"),
        ),
        (
            "a region outside the published character class",
            format!("arn:aws:codeconnections:EU_WEST_1:{account}:connection/c1"),
        ),
    ];

    for (label, connection_arn) in &refused {
        let (status, body) = call(
            &router,
            "CreateRepositoryLink",
            serde_json::json!({
                "ConnectionArn": connection_arn,
                "OwnerId": "an-owner",
                "RepositoryName": format!("repo-{label}"),
            }),
        )
        .await;
        assert_validation_message(
            &format!("CreateRepositoryLink: {label}"),
            &expected,
            status,
            &body,
        );

        let (status, body) = call(
            &router,
            "UpdateRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id, "ConnectionArn": connection_arn }),
        )
        .await;
        assert_validation_message(
            &format!("UpdateRepositoryLink: {label}"),
            &expected,
            status,
            &body,
        );
    }

    // The refused rows wrote nothing: the link still reads back as it was created.
    let (status, read_back) = call(
        &router,
        "GetRepositoryLink",
        serde_json::json!({ "RepositoryLinkId": link_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetRepositoryLink: expected 200, got {status} {read_back}"
    );
    assert_eq!(
        read_back["RepositoryLinkInfo"], link,
        "a refused row must change nothing, got {read_back}"
    );
}

/// The other side of the same rule: an ARN the connection-scoped expression ADMITS is not
/// refused as invalid input — it is RESOLVED, and reports the resolution's own outcome.
///
/// Rows span the expression's branches (this service's namespace and the legacy one, a
/// second partition, another region, another account, an id spelled as a uuid and one
/// spelled as a word), so the tightening refuses malformed ARNs without narrowing the set
/// of ARNs the service accepts.
#[tokio::test]
async fn an_arn_the_expression_admits_is_resolved_rather_than_refused() {
    let router = router();
    let account = engine::emulator_account_id();
    let live = create(&router, "live", "GitLab").await;

    let admitted = [
        (
            "a connection id this store never minted",
            format!(
                "arn:aws:codeconnections:{REGION}:{account}:connection/\
                 11111111-2222-3333-4444-555555555555"
            ),
        ),
        (
            "the legacy codestar-connections namespace",
            live.replace("codeconnections", "codestar-connections"),
        ),
        (
            "another partition",
            live.replace("arn:aws:", "arn:aws-us-gov:"),
        ),
        ("another region", live.replace(REGION, "ca-central-1")),
        ("another account", live.replace(account, "314159265358")),
        (
            "an id spelled as a word",
            format!("arn:aws:codeconnections:{REGION}:{account}:connection/some-connection"),
        ),
    ];

    for (label, connection_arn) in &admitted {
        let (status, body) = call(
            &router,
            "CreateRepositoryLink",
            serde_json::json!({
                "ConnectionArn": connection_arn,
                "OwnerId": "an-owner",
                "RepositoryName": format!("repo-{label}"),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a connection that is not there, got {status} {body}"
        );
        let kind = body["__type"].as_str().unwrap_or_default();
        assert!(
            kind.ends_with("InvalidInputException"),
            "case {label:?}: expected the resolution's own refusal, got {body}"
        );
    }

    // And the live connection is accepted and reads back through `GetRepositoryLink`.
    let (status, created) = call(
        &router,
        "CreateRepositoryLink",
        serde_json::json!({
            "ConnectionArn": live,
            "OwnerId": "an-owner",
            "RepositoryName": "the-repo",
        }),
    )
    .await;
    assert_eq!(status, 200, "the live connection: got {status} {created}");
    let link_id = created["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no RepositoryLinkId: {created}"))
        .to_string();
    let (status, read_back) = call(
        &router,
        "GetRepositoryLink",
        serde_json::json!({ "RepositoryLinkId": link_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetRepositoryLink: expected 200, got {status} {read_back}"
    );
    assert_eq!(
        read_back["RepositoryLinkInfo"], created["RepositoryLinkInfo"],
        "the created link must read back unchanged, got {read_back}"
    );
}
