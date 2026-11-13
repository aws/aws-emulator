// SPDX-License-Identifier: CC0-1.0

//! Turning single sign-on on and off AS A DIRECTORY ACCOUNT — the `UserName`/`Password` pair
//! `EnableSso` and `DisableSso` take — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing a RESPONSE claims is read from the state
//! struct: whether single sign-on is on is read back through `DescribeDirectories`, so an
//! answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: a credential pair such a call carries is AUTHENTICATED against the
//! directory — a pair naming an account the directory holds, presented with the password
//! that account's password was set to, is admitted and the call takes effect, while a pair
//! the directory does not hold is refused with the modelled
//! `AuthenticationFailedException`. The password an account's password was set to is the
//! `Password` the create that made it was called with, and after a reset the password that
//! reset set.
//!
//! It is asserted over a TABLE rather than one recorded call, along what the rule
//! generalises over: the create that made the account (each of the two, whose administrator
//! accounts are named differently and whose passwords differ), the spelling of the account
//! name (Active Directory compares one case-insensitively), which half of the pair is wrong
//! (the account, the password, or one half missing), and which of the two operations that
//! take the pair is called.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{create_directory, describe_one};
use crate::harness::{call, ok, service, string};
use crate::microsoft_ad::create_microsoft_ad;

/// How long a directory may take to report itself in service before a case fails: far
/// longer than any span this service publishes, and well inside what a client's waiter
/// allows.
const IN_SERVICE_DEADLINE: Duration = Duration::from_secs(20);

/// How often that wait asks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The stage a directory whose create has finished reports: the `Active` value of the
/// modelled `DirectoryStage` enum.
const STAGE_ACTIVE: &str = "Active";

/// The administrator account a Simple AD directory is created with, from
/// `CreateDirectoryRequest$Password`'s own documentation: "The directory creation process
/// creates a directory administrator account with the user name `Administrator`".
const SIMPLE_AD_ADMINISTRATOR: &str = "Administrator";

/// The administrator account a Managed Microsoft AD directory is created with, from
/// `CreateMicrosoftADRequest$Password`'s own documentation: "The password for the default
/// administrative user named `Admin`".
const MICROSOFT_AD_ADMINISTRATOR: &str = "Admin";

/// The sentence this service states when it could not authenticate the account a call
/// carried: the text the modelled error shape's own documentation states for the class
/// (`smithy_query(query="doc", shape="AuthenticationFailedException")`).
const AUTHENTICATION_FAILED: &str = "An authentication error occurred.";

/// One row: a directory each create makes, with the account its own create documents and the
/// password that create was called with.
struct DirectoryRow {
    case: &'static str,
    operation: &'static str,
    name: &'static str,
    administrator: &'static str,
    password: &'static str,
    alias: &'static str,
}

/// The directories this rule is asserted over: one per create that makes an administrator
/// account, each with its own account name and its OWN password, so no case can pass by
/// authenticating against another directory's credential.
fn directory_rows() -> Vec<DirectoryRow> {
    vec![
        DirectoryRow {
            case: "a Simple AD directory",
            operation: "CreateDirectory",
            name: "sso-simple.example.com",
            administrator: SIMPLE_AD_ADMINISTRATOR,
            password: "Sim2Real-Simple-Pw1!",
            alias: "sso-simple",
        },
        DirectoryRow {
            case: "a Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            name: "sso-managed.example.com",
            administrator: MICROSOFT_AD_ADMINISTRATOR,
            password: "Sim2RealManagedPw2",
            alias: "sso-managed",
        },
    ]
}

/// The create body for `row`, through whichever create operation it names.
fn create_body(row: &DirectoryRow) -> Value {
    match row.operation {
        "CreateMicrosoftAD" => json!({
            "Name": row.name,
            "Password": row.password,
            "VpcSettings": {
                "VpcId": "vpc-0a1b2c3d",
                "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
            }
        }),
        _ => json!({ "Name": row.name, "Password": row.password, "Size": "Small" }),
    }
}

/// Create the row's directory, wait until it is in service and assign its alias — single
/// sign-on needs the access URL an alias provides, and an alias can only be assigned to a
/// directory that is in service — and return the id.
async fn create_aliased_directory(router: &MultiServiceRouter, row: &DirectoryRow) -> String {
    let id = match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, create_body(row), row.case).await,
        _ => create_directory(router, create_body(row), row.case).await,
    };
    in_service(router, &id, row.case).await;
    ok(
        router,
        "CreateAlias",
        json!({ "DirectoryId": &id, "Alias": row.alias }),
        row.case,
    )
    .await;
    id
}

/// Wait until `DescribeDirectories` reports `directory_id` in service, the way a client with
/// a waiter does: ask, and ask again until the stage the create owes has settled or a
/// generous deadline passes.
async fn in_service(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let started = Instant::now();
    loop {
        let described = describe_one(router, directory_id, case).await;
        let stage = string(&described, "Stage", case);
        if stage == STAGE_ACTIVE {
            return;
        }
        assert!(
            started.elapsed() < IN_SERVICE_DEADLINE,
            "case {case:?}: expected {directory_id} to report itself in service within \
             {IN_SERVICE_DEADLINE:?}, last observed stage {stage:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `DescribeDirectories` reports single sign-on as on for `directory_id`.
async fn sso_enabled(router: &MultiServiceRouter, directory_id: &str, case: &str) -> bool {
    let described = describe_one(router, directory_id, case).await;
    described["SsoEnabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean SsoEnabled, got {described}"))
}

/// An `EnableSso`/`DisableSso` body carrying whichever halves of the credential pair the
/// case supplies.
fn with_credentials(directory_id: &str, user_name: Option<&str>, password: Option<&str>) -> Value {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(user_name) = user_name {
        body["UserName"] = json!(user_name);
    }
    if let Some(password) = password {
        body["Password"] = json!(password);
    }
    body
}

/// Assert `operation` with those credentials is refused as an authentication failure,
/// disclosing neither half of the pair.
async fn refused_as_unauthenticated(
    router: &MultiServiceRouter,
    operation: &str,
    directory_id: &str,
    user_name: Option<&str>,
    password: Option<&str>,
    case: &str,
) {
    let (status, parsed) = call(
        router,
        operation,
        with_credentials(directory_id, user_name, password),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to refuse the credentials, got {status} with \
         {parsed}"
    );
    assert_eq!(
        parsed["__type"],
        json!("AuthenticationFailedException"),
        "case {case:?}: expected the modelled authentication refusal, got {parsed}"
    );
    let reported = string(&parsed, "RequestId", case);
    let stated = parsed["Message"].as_str().unwrap_or_default();
    let suffix = format!(" : RequestId: {reported}");
    assert_eq!(
        stated,
        format!("{AUTHENTICATION_FAILED}{suffix}"),
        "case {case:?}: expected the authentication-failure sentence, got {parsed}"
    );
    // Neither member is echoed back: a refusal discloses nothing the caller sent. Scoped to
    // what the refusal AUTHORS — its diagnostic and its error type — because the request id
    // is minted per call and a short credential can turn up in its hex digits by chance.
    let diagnostic = stated.strip_suffix(&suffix).unwrap_or(stated);
    let disclosed = format!("{diagnostic}{}", parsed["__type"]);
    if let Some(password) = password {
        assert!(
            !disclosed.contains(password),
            "case {case:?}: expected the refusal not to echo the password, got {parsed}"
        );
    }
    if let Some(user_name) = user_name {
        assert!(
            !disclosed.contains(user_name),
            "case {case:?}: expected the refusal not to echo the account name, got {parsed}"
        );
    }
}

/// RULE: the directory's own administrator account, presented with the password its create
/// was called with, is authenticated — so the documented enable path works and the flag the
/// reader reports follows it, in both directions.
#[tokio::test]
async fn the_directorys_own_administrator_account_enables_and_disables_single_sign_on() {
    let router = service();

    let rows = directory_rows();
    let mut ids = Vec::new();
    for row in &rows {
        ids.push(create_aliased_directory(&router, row).await);
    }

    for (row, id) in rows.iter().zip(&ids) {
        // An Active Directory account name is compared case-insensitively, so every
        // spelling of the administrator's name names the same account — and the enable is
        // repeatable, since it asks for a state rather than consuming anything.
        for spelling in [
            row.administrator.to_string(),
            row.administrator.to_lowercase(),
            row.administrator.to_uppercase(),
        ] {
            let case = format!("{}: EnableSso as {spelling:?}", row.case);
            let answer = ok(
                &router,
                "EnableSso",
                with_credentials(id, Some(&spelling), Some(row.password)),
                &case,
            )
            .await;
            assert_eq!(
                answer,
                json!({}),
                "case {case:?}: EnableSso has no modelled output members, got {answer}"
            );
            assert!(
                sso_enabled(&router, id, &case).await,
                "case {case:?}: expected the authenticated enable to be reported"
            );

            // ...and the same credentials turn it back off, which is the half of the
            // teardown path the pair is otherwise unreachable through.
            let case = format!("{}: DisableSso as {spelling:?}", row.case);
            ok(
                &router,
                "DisableSso",
                with_credentials(id, Some(&spelling), Some(row.password)),
                &case,
            )
            .await;
            assert!(
                !sso_enabled(&router, id, &case).await,
                "case {case:?}: expected the authenticated disable to be reported"
            );
        }
    }
}

/// RULE: a pair the directory does NOT hold is refused — whichever half is wrong, whichever
/// operation takes it — and a refused call changes nothing.
#[tokio::test]
async fn a_pair_the_directory_does_not_hold_is_refused() {
    let router = service();

    let rows = directory_rows();
    let mut ids = Vec::new();
    for row in &rows {
        ids.push(create_aliased_directory(&router, row).await);
    }

    for (index, (row, id)) in rows.iter().zip(&ids).enumerate() {
        // The OTHER row's directory, so a case can present a credential pair that is
        // genuinely held — by another directory.
        let other = &rows[(index + 1) % rows.len()];
        let wrong: Vec<(String, Option<String>, Option<String>)> = vec![
            (
                "the right account with another password".to_string(),
                Some(row.administrator.to_string()),
                Some(format!("{}x", row.password)),
            ),
            (
                "the right account with another directory's password".to_string(),
                Some(row.administrator.to_string()),
                Some(other.password.to_string()),
            ),
            (
                "an account the directory holds none of".to_string(),
                Some("ssoadmin".to_string()),
                Some(row.password.to_string()),
            ),
            (
                "the other directory type's administrator name".to_string(),
                Some(other.administrator.to_string()),
                Some(row.password.to_string()),
            ),
            (
                "the account with no password presented".to_string(),
                Some(row.administrator.to_string()),
                None,
            ),
            (
                "the password with no account presented".to_string(),
                None,
                Some(row.password.to_string()),
            ),
        ];

        for (class, user_name, password) in &wrong {
            for operation in ["EnableSso", "DisableSso"] {
                let case = format!("{}: {operation} with {class}", row.case);
                refused_as_unauthenticated(
                    &router,
                    operation,
                    id,
                    user_name.as_deref(),
                    password.as_deref(),
                    &case,
                )
                .await;
                assert!(
                    !sso_enabled(&router, id, &case).await,
                    "case {case:?}: expected a refused call to record nothing"
                );
            }
        }
    }
}

/// RULE: the password an account's credentials carry is the one its password was last SET
/// to — so a reset moves the credential with it: the password the reset set authenticates
/// and the one it replaced no longer does.
#[tokio::test]
async fn a_reset_moves_the_account_credential_with_it() {
    let router = service();

    let rows = directory_rows();
    let mut ids = Vec::new();
    for row in &rows {
        ids.push(create_aliased_directory(&router, row).await);
    }
    // A password is set in the directory's own domain, so a reset needs the directory in
    // service, which the fixture above has already waited for.

    for (row, id) in rows.iter().zip(&ids) {
        let case = format!("{}: after a reset", row.case);
        let reset_to = format!("{}-Reset9", row.password);
        ok(
            &router,
            "ResetUserPassword",
            json!({
                "DirectoryId": id,
                "UserName": row.administrator,
                "NewPassword": &reset_to,
            }),
            &case,
        )
        .await;

        // The password the create was called with is no longer the account's.
        refused_as_unauthenticated(
            &router,
            "EnableSso",
            id,
            Some(row.administrator),
            Some(row.password),
            &format!("{case}: the password the reset replaced"),
        )
        .await;
        assert!(
            !sso_enabled(&router, id, &case).await,
            "case {case:?}: expected the refused enable to record nothing"
        );

        // The password the reset set is.
        ok(
            &router,
            "EnableSso",
            with_credentials(id, Some(row.administrator), Some(&reset_to)),
            &format!("{case}: the password the reset set"),
        )
        .await;
        assert!(
            sso_enabled(&router, id, &case).await,
            "case {case:?}: expected the enable with the reset password to be reported"
        );
    }
}
