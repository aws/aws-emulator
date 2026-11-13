// SPDX-License-Identifier: CC0-1.0

//! What becomes of a credential ALREADY vended when the pool that vended it changes.
//!
//! RULE: what `GetCredentialsForIdentity` hands back is an `sts:AssumeRoleWithWebIdentity`
//! session of the role the pool resolved, and that session is keyed by ITSELF — its own
//! credential and its own `Expiration` — not by the pool or the identity record the exchange
//! went through. So no later call of the identity pool's own lifecycle revokes it: the
//! session keeps resolving to the same assumed-role principal, and the policies in force on
//! that role keep deciding what a request signed with it may do, until the session expires.
//! A pool the lifecycle removed stops further VENDS, which is a different effect from
//! revoking a credential already in a caller's hands.
//!
//! The rows differ along what the rule generalises over: WHICH call of the pool's or the
//! identity's lifecycle follows the vend — the pool deleted, the identity the session was
//! vended for deleted, that identity merged away into another user, the pool's roles
//! repointed at a different role, the pool told to stop admitting unauthenticated identities
//! — beside a control row where nothing happens at all. Each row reads the session back
//! several times, because a read of a session is not a consumption of it.
//!
//! The harness is `vended_role_session`'s: this service and the token service on ONE shared
//! engine behind the same `MultiServiceRouter` the production router builds, with the IAM
//! records the decision reads seeded into that same engine. Reading WHO a vended credential
//! speaks for is what makes this suite's assertions possible: `sts:GetCallerIdentity` answers
//! from the token service's session registry, and the account's own role-session record is
//! what a service's authorization gate resolves an access key id through.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;

use crate::vended_role_session::{element, expected_session_arn, trust_policy, vended, Emulator};

/// The `Logins` provider name a token this service minted is presented back under
/// (`GetCredentialsForIdentityInput$Logins`).
const COGNITO_PROVIDER: &str = "cognito-identity.amazonaws.com";

/// The `DeveloperProviderName` of the pool the merge row needs, since a merge is stated over
/// developer user identifiers.
const DEVELOPER_PROVIDER: &str = "my.dev_provider-1";

/// The developer user identifier of the user whose session the merge row vends, and which the
/// merge gives up.
const MERGED_AWAY_USER: &str = "the-merged-away-user";

/// The developer user identifier of the user the merge row keeps.
const SURVIVING_USER: &str = "the-surviving-user";

/// How many times a row reads its session back after the lifecycle call. More than one: a
/// session is not consumed by being resolved, so every read must answer the same principal.
const READS_AFTER: usize = 3;

/// Which call of the pool's or the identity's own lifecycle follows the vend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    /// `DeleteIdentityPool` on the pool the session was vended by.
    PoolDeleted,
    /// `DeleteIdentities` naming the identity the session was vended for.
    IdentityDeleted,
    /// `MergeDeveloperIdentities` giving that identity's logins to another user.
    IdentityMergedAway,
    /// `SetIdentityPoolRoles` pointing the pool at a different role.
    RolesRepointed,
    /// `UpdateIdentityPool` turning `AllowUnauthenticatedIdentities` off.
    GuestsDisallowed,
    /// Nothing at all: the control row.
    Nothing,
}

impl Lifecycle {
    /// Whether the row's identity has to be a developer-authenticated one, which only the
    /// merge is stated over.
    fn needs_developer_identity(self) -> bool {
        self == Self::IdentityMergedAway
    }
}

/// The rows: one per lifecycle call, plus the control.
fn rows() -> Vec<(&'static str, Lifecycle)> {
    vec![
        ("the identity pool is deleted", Lifecycle::PoolDeleted),
        (
            "the identity the session was vended for is deleted",
            Lifecycle::IdentityDeleted,
        ),
        (
            "that identity is merged away into another developer user",
            Lifecycle::IdentityMergedAway,
        ),
        (
            "the pool's roles are repointed at another role",
            Lifecycle::RolesRepointed,
        ),
        (
            "the pool stops admitting unauthenticated identities",
            Lifecycle::GuestsDisallowed,
        ),
        ("nothing happens to the pool", Lifecycle::Nothing),
    ]
}

/// One pool for a row, carrying a developer provider when the row's lifecycle call needs one.
async fn pool_for(emulator: &Emulator, name: &str, lifecycle: Lifecycle) -> String {
    let mut request = json!({
        "IdentityPoolName": name,
        "AllowUnauthenticatedIdentities": true,
    });
    if lifecycle.needs_developer_identity() {
        request["DeveloperProviderName"] = json!(DEVELOPER_PROVIDER);
    }
    let created = emulator.ok("CreateIdentityPool", &request).await;
    created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
        .to_string()
}

/// The identity a row vends for, with the `GetCredentialsForIdentity` request that vends it:
/// a developer-authenticated identity presents the token its own issuance handed back, and a
/// guest identity presents no login at all.
async fn identity_and_vend_request(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    lifecycle: Lifecycle,
) -> (String, Value) {
    if lifecycle.needs_developer_identity() {
        let answered = emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &json!({
                    "IdentityPoolId": pool_id,
                    "Logins": {DEVELOPER_PROVIDER: MERGED_AWAY_USER},
                }),
            )
            .await;
        let identity_id = answered["IdentityId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a mint answers an IdentityId: {answered}"))
            .to_string();
        let token = answered["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a mint answers a Token: {answered}"))
            .to_string();
        let request = json!({"IdentityId": &identity_id, "Logins": {COGNITO_PROVIDER: token}});
        (identity_id, request)
    } else {
        let identity_id = emulator.identity(pool_id, None).await;
        let request = json!({"IdentityId": &identity_id});
        (identity_id, request)
    }
}

/// Make the row's lifecycle call, each as a client makes it.
async fn make_lifecycle_call(
    emulator: &Emulator,
    lifecycle: Lifecycle,
    pool_id: &str,
    pool_name: &str,
    identity_id: &str,
    repointed_role: &str,
) {
    match lifecycle {
        Lifecycle::PoolDeleted => {
            emulator
                .ok("DeleteIdentityPool", &json!({"IdentityPoolId": pool_id}))
                .await;
        }
        Lifecycle::IdentityDeleted => {
            emulator
                .ok(
                    "DeleteIdentities",
                    &json!({"IdentityIdsToDelete": [identity_id]}),
                )
                .await;
        }
        Lifecycle::IdentityMergedAway => {
            // The user the merge keeps has to exist before the merge names it.
            emulator
                .ok(
                    "GetOpenIdTokenForDeveloperIdentity",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "Logins": {DEVELOPER_PROVIDER: SURVIVING_USER},
                    }),
                )
                .await;
            emulator
                .ok(
                    "MergeDeveloperIdentities",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "DeveloperProviderName": DEVELOPER_PROVIDER,
                        "SourceUserIdentifier": SURVIVING_USER,
                        "DestinationUserIdentifier": MERGED_AWAY_USER,
                    }),
                )
                .await;
        }
        Lifecycle::RolesRepointed => {
            emulator
                .ok(
                    "SetIdentityPoolRoles",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "Roles": {
                            "authenticated": repointed_role,
                            "unauthenticated": repointed_role,
                        },
                    }),
                )
                .await;
        }
        Lifecycle::GuestsDisallowed => {
            emulator
                .ok(
                    "UpdateIdentityPool",
                    &json!({
                        "IdentityPoolId": pool_id,
                        "IdentityPoolName": pool_name,
                        "AllowUnauthenticatedIdentities": false,
                    }),
                )
                .await;
        }
        Lifecycle::Nothing => {}
    }
}

/// The assumed-role ARN the token service reports for `access_key_id`.
async fn reported_principal(emulator: &Emulator, case: &str, access_key_id: &str) -> String {
    let (status, reported) = emulator.caller_identity(access_key_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCallerIdentity under the vended credential expected 200, got \
         {status} with {reported}"
    );
    element(&reported, "Arn")
}

/// RULE: a session already vended survives every call of its pool's own lifecycle, still
/// speaking for the role it was minted of — in the token service's registry and in the
/// account's role-session record alike.
#[tokio::test]
async fn a_vended_session_survives_its_pools_lifecycle() {
    let emulator = Emulator::new();

    for (index, (case, lifecycle)) in rows().into_iter().enumerate() {
        let pool_name = format!("outlives-{index}");
        let pool_id = pool_for(&emulator, &pool_name, lifecycle).await;
        let role_name = format!("outlives-role-{index}");
        let role_arn = emulator.seed_role(&role_name, Some(trust_policy(&pool_id)));
        // One role for both flows, so the row's session is of the same role whether its
        // exchange presented a login or not.
        emulator
            .ok(
                "SetIdentityPoolRoles",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
                }),
            )
            .await;
        let repointed_role = emulator.seed_role(
            &format!("outlives-other-role-{index}"),
            Some(trust_policy(&pool_id)),
        );

        let (identity_id, vend_request) =
            identity_and_vend_request(&emulator, case, &pool_id, lifecycle).await;
        let issued = emulator
            .ok("GetCredentialsForIdentity", &vend_request)
            .await;
        let (access_key_id, _) = vended(&issued);
        let expected = expected_session_arn(&role_name);

        // The control: before the lifecycle call, the session speaks for the role the pool
        // resolved. Everything the row asserts afterwards is about THAT session.
        let before = reported_principal(&emulator, case, access_key_id).await;
        assert_eq!(
            before, expected,
            "case {case:?}: before the lifecycle call the vend must be a session of \
             {role_name}, got {before}"
        );

        make_lifecycle_call(
            &emulator,
            lifecycle,
            &pool_id,
            &pool_name,
            &identity_id,
            &repointed_role,
        )
        .await;

        for read in 1..=READS_AFTER {
            let after = reported_principal(&emulator, case, access_key_id).await;
            assert_eq!(
                after, expected,
                "case {case:?}: read {read} of {READS_AFTER}: the vended session must still \
                 speak for {expected}, got {after}"
            );
        }

        // The other reading of the same question: the account's role-session record, which
        // is what a service's authorization gate resolves an access key id through — so what
        // it names is the role a request signed with this credential is judged against.
        let now = aws_smithy_types::DateTime::from(SystemTime::now());
        let held = emulator
            .engine
            .state::<iam::ComAmazonawsIamAwsidentitymanagementv20100508State>();
        let record = held.lock().role_session(access_key_id, now).cloned();
        let record = record.unwrap_or_else(|| {
            panic!(
                "case {case:?}: the account must still hold a role session for the vended \
                 credential, so a request signed with it is judged against {role_arn}"
            )
        });
        assert_eq!(
            record.role_arn, role_arn,
            "case {case:?}: expected the record to place {role_arn} behind the credential, \
             got {:?}",
            record.role_arn
        );
        assert_eq!(
            record.session_arn, expected,
            "case {case:?}: expected the record to name the session {expected}, got {:?}",
            record.session_arn
        );
    }
}

/// RULE, the other half: what a pool delete stops is a further VEND. A pool this service no
/// longer holds resolves no role for a new exchange, so the next
/// `GetCredentialsForIdentity` for one of its identities is refused the modelled
/// `ResourceNotFoundException` naming that pool — while the session vended before the delete
/// goes on speaking for its role.
#[tokio::test]
async fn a_deleted_pool_stops_further_vends_without_revoking_the_last_one() {
    let emulator = Emulator::new();
    let pool_id = pool_for(&emulator, "outlives-stops-vends", Lifecycle::PoolDeleted).await;
    let role_name = "outlives-stops-vends-role";
    let role_arn = emulator.seed_role(role_name, Some(trust_policy(&pool_id)));
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {"authenticated": &role_arn, "unauthenticated": &role_arn},
            }),
        )
        .await;
    let identity_id = emulator.identity(&pool_id, None).await;
    let issued = emulator
        .ok(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    let (access_key_id, _) = vended(&issued);

    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &pool_id}))
        .await;

    // A NEW exchange for the same identity has no pool to resolve a role from. What the
    // refusal is WORDED as is asserted here; the HTTP status this service answers a
    // missing resource with is pinned by `deleted_pool_and_developer_user_refusals`.
    let (status, refused) = emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    assert!(
        status >= 400,
        "a vend against a deleted pool must be refused, got {status} with {refused}"
    );
    let code = refused["__type"].as_str().unwrap_or_default();
    assert!(
        code.ends_with("ResourceNotFoundException"),
        "expected the missing-pool refusal, got {code:?} with {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some(format!("IdentityPool '{pool_id}' not found.").as_str()),
        "expected the refusal to name the pool that is gone, got {refused}"
    );

    // And the session vended before the delete is untouched by it.
    let reported = reported_principal(&emulator, "after the pool went", access_key_id).await;
    assert_eq!(
        reported,
        expected_session_arn(role_name),
        "the session vended before the delete must still speak for {role_name}, got {reported}"
    );
}
