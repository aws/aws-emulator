// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three rules of the ElastiCache RBAC user surface:
//!
//! * the EXPANDED access string the service stores and reports — the rule list with its
//!   command baseline stated outright;
//! * `DeleteUser`: the user is answered `deleting` and is then GONE, so every later
//!   lookup of it is the modelled `UserNotFoundFault`;
//! * the `<RequestId>` element an awsQuery `<ErrorResponse>` envelope carries, on every
//!   error this service answers with, whichever layer produced it.
//!
//! Every assertion goes over the wire, through the same router the production emulator
//! mounts, and nothing reaches into the service's state struct — so a response that is
//! not computed from persisted state cannot pass. The request builder and the XML reader
//! are the shared ones in `tests/all/harness.rs`.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (whether and how the rule list states its baseline, where its first
//! command rule sits, which engine and authentication the deleted user had, which
//! operation and which layer produced the error), so no single recorded request is what
//! makes them pass.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// One access-string row: the case name, the `AccessString` the create sends, and the
/// expanded form the service must store and report for it.
type AccessStringCase = (&'static str, &'static str, &'static str);

/// The access-string normalization table.
///
/// The axis is the one the rule generalises over: whether the submitted rule list states
/// its command baseline, which of the engine's four ways it states it, and where the
/// first rule the baseline governs sits in the list. `on ~sim2real:* +@read +@write` is
/// one row.
///
/// The four baseline spellings are `+@all` / `-@all` and the engine's keyword aliases
/// `allcommands` / `nocommands`, which are in this service's
/// `state::ACCESS_STRING_KEYWORDS` catalog of the engine's keyword rules.
const ACCESS_STRING_CASES: &[AccessStringCase] = &[
    (
        "two category grants, no baseline",
        "on ~sim2real:* +@read +@write",
        "on ~sim2real:* -@all +@read +@write",
    ),
    (
        "one command grant after a channel pattern, no baseline",
        "on ~app:* &news:* +get",
        "on ~app:* &news:* -@all +get",
    ),
    (
        "a revoke and nothing else, no baseline",
        "on ~* -@admin",
        "on ~* -@all -@admin",
    ),
    (
        "no command rule at all: the baseline is the whole command half",
        "on ~*",
        "on ~* -@all",
    ),
    (
        "the baseline comes first, before the key pattern",
        "on +@read ~app:*",
        "on -@all +@read ~app:*",
    ),
    (
        "deny-all already stated: expands to itself",
        "on ~app:* -@all +@read",
        "on ~app:* -@all +@read",
    ),
    (
        "allow-all already stated: expands to itself",
        "on ~* &* +@all",
        "on ~* &* +@all",
    ),
    (
        "the engine's allcommands alias states it",
        "on ~* allcommands",
        "on ~* allcommands",
    ),
    (
        "the engine's nocommands alias states it",
        "on ~* nocommands +get",
        "on ~* nocommands +get",
    ),
    (
        // A selector carries a permission set of its own, so its rules are not rules of
        // the list around it: the list below names none the baseline governs, and keeps
        // the selector exactly as sent.
        "a selector's own command rules are not the list's",
        "on ~* (+get ~key1)",
        "on ~* (+get ~key1) -@all",
    ),
    (
        "runs of whitespace around the rules",
        "  on   ~app:*   +@read  ",
        "on ~app:* -@all +@read",
    ),
];

/// Create one user with `AccessString` and the members every row shares.
async fn create_user(emulator: &Emulator, id: &str, access_string: &str) -> Reply {
    emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "valkey"),
                ("AccessString", access_string),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await
}

/// The `AccessString` a reply reports for its single user.
fn reported_access_string(case: &str, reply: &Reply) -> String {
    reply
        .text("AccessString")
        .unwrap_or_else(|| panic!("case {case}: the reply reports no AccessString:\n{}", reply.body))
        .to_string()
}

#[tokio::test]
async fn the_stored_access_string_states_its_command_baseline() {
    let emulator = Emulator::new();
    for (index, (case, submitted, expanded)) in ACCESS_STRING_CASES.iter().enumerate() {
        // One user per row, so each row reads back its own stored access string.
        let id = format!("acl-user-{index}");
        let created = create_user(&emulator, &id, submitted).await;
        assert_eq!(
            created.status, 200,
            "case {case}: CreateUser must answer 200, got {} {}",
            created.status, created.body
        );
        let echoed = reported_access_string(case, &created);
        assert_eq!(
            &echoed, expanded,
            "case {case}: CreateUser must echo the expanded access string {expanded:?}, got \
             {echoed:?}"
        );

        // The read path reports the same stored document, so an audit that reads the
        // user back sees the permissions the service holds.
        let described = emulator.call("DescribeUsers", &params(&[("UserId", &id)])).await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeUsers must answer 200, got {} {}",
            described.status, described.body
        );
        let read_back = reported_access_string(case, &described);
        assert_eq!(
            &read_back, expanded,
            "case {case}: DescribeUsers must report the stored access string {expanded:?}, got \
             {read_back:?}"
        );
    }
}

/// One deletion row: the case name, and the members that make the user differ from the
/// others (its id, its engine, and the form pairs that say how it authenticates).
type DeletionCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The deletion table: one row per kind of user, so the rule under test is not tied to
/// the engine a user runs or the way it authenticates.
const DELETION_CASES: &[DeletionCase] = &[
    (
        "a redis user with no password",
        "doomed-redis",
        "redis",
        &[("NoPasswordRequired", "true")],
    ),
    (
        "a valkey user with two passwords",
        "doomed-valkey",
        "valkey",
        &[
            ("Passwords.member.1", "aVeryLongPassword1"),
            ("Passwords.member.2", "aVeryLongPassword2"),
        ],
    ),
    (
        "a user authenticating with IAM",
        "doomed-iam",
        "valkey",
        &[("AuthenticationMode.Type", "iam")],
    ),
];

/// Create the user one deletion row describes.
async fn create_deletion_case(
    emulator: &Emulator,
    (case, id, engine, authentication): &DeletionCase,
) -> Reply {
    let mut pairs = params(&[
        ("UserId", *id),
        ("UserName", *id),
        ("Engine", *engine),
        ("AccessString", "on ~* +@all"),
    ]);
    pairs.extend(params(authentication));
    let created = emulator.call("CreateUser", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUser must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

#[tokio::test]
async fn a_deleted_user_is_reported_deleting_and_then_is_gone() {
    // The wait below must out-last the span the service publishes, or a test would read
    // a user the clock had not removed yet and report it as a failure of the handler.
    assert!(
        PAST_TRANSITION > state::USER_DELETION_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the user deletion span \
         ({:?}) this service publishes",
        state::USER_DELETION_TRANSITION
    );
    let emulator = Emulator::new();
    for row in DELETION_CASES {
        let (case, id, _, _) = row;
        let created = create_deletion_case(&emulator, row).await;

        let deleted = emulator.call("DeleteUser", &params(&[("UserId", id)])).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: DeleteUser must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        // The delete answers with the user it removed, in the one status that says it is
        // leaving; everything else about it is what the create stored.
        assert_eq!(
            deleted.text("Status"),
            Some(state::USER_STATUS_DELETING),
            "case {case}: DeleteUser must report the user {:?}, got {:?}",
            state::USER_STATUS_DELETING,
            deleted.text("Status")
        );
        for member in ["UserId", "UserName", "Engine", "AccessString", "ARN"] {
            assert_eq!(
                deleted.text(member),
                created.text(member),
                "case {case}: DeleteUser must report the stored {member}, got {:?}",
                deleted.text(member)
            );
        }

        // The read that follows is answered once the removal the clock owed has happened,
        // so the lookup is the modelled not-found — the answer a revoked credential must
        // get.
        let gone = emulator.call("DescribeUsers", &params(&[("UserId", id)])).await;
        assert_eq!(
            (gone.status, gone.error_code()),
            (404, "UserNotFound"),
            "case {case}: a deleted user must describe as 404 UserNotFound, got {} {}",
            gone.status,
            gone.body
        );
        // It is gone from the unfiltered listing too, not just from the lookup.
        let listed = emulator.call("DescribeUsers", &params(&[])).await;
        assert!(
            !listed.all("UserId").contains(id),
            "case {case}: a deleted user must not be listed, got {:?}",
            listed.all("UserId")
        );
        // And the id it held is free again: the same user can be created anew.
        let recreated = create_deletion_case(&emulator, row).await;
        assert_eq!(
            recreated.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {case}: a user created after the delete must be active, got {:?}",
            recreated.text("Status")
        );
    }
    // The built-in user every account has is untouched by all of that.
    let default_user = emulator
        .call(
            "DescribeUsers",
            &params(&[("UserId", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        (default_user.status, default_user.text("Status")),
        (200, Some(state::USER_STATUS_ACTIVE)),
        "the built-in user must still be active, got {} {}",
        default_user.status,
        default_user.body
    );
}

#[tokio::test]
async fn a_delete_the_service_refuses() {
    let emulator = Emulator::new();
    // Rows that need no setup: the user does not exist, and the built-in user no call
    // creates and none deletes.
    let standalone: &[(&str, &str, u16, &str)] = &[
        ("no such user", "absent-user", 404, "UserNotFound"),
        (
            "the built-in default user",
            state::DEFAULT_USER_ID,
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, id, status, code) in standalone {
        let refused = emulator.call("DeleteUser", &params(&[("UserId", id)])).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
    }

    // A user already on its way out is not in active state, which is the modelled state
    // fault — and the second delete must not shorten or restart the first one.
    let row = &DELETION_CASES[0];
    create_deletion_case(&emulator, row).await;
    let first = emulator
        .call("DeleteUser", &params(&[("UserId", row.1)]))
        .await;
    assert_eq!(
        first.status, 200,
        "the first delete must answer 200, got {} {}",
        first.status, first.body
    );
    let second = emulator
        .call("DeleteUser", &params(&[("UserId", row.1)]))
        .await;
    assert_eq!(
        (second.status, second.error_code()),
        (400, "InvalidUserState"),
        "deleting a user that is already deleting must be the modelled state fault, got {} {}",
        second.status,
        second.body
    );
    // Once the removal falls due the user is gone, so the delete is a not-found.
    std::thread::sleep(PAST_TRANSITION);
    let third = emulator
        .call("DeleteUser", &params(&[("UserId", row.1)]))
        .await;
    assert_eq!(
        (third.status, third.error_code()),
        (404, "UserNotFound"),
        "deleting a user that is gone must be the modelled not-found, got {} {}",
        third.status,
        third.body
    );
}

/// The id length table: the maximum length the service publishes
/// (`state::MAX_USER_ID_LEN` — ElastiCache documents a user id as 1 to 40 characters),
/// the length one under it, the maximum ITSELF (which is accepted), and lengths past it.
#[tokio::test]
async fn a_user_id_past_the_length_bound_is_refused() {
    let emulator = Emulator::new();
    let maximum = state::MAX_USER_ID_LEN;
    for length in [1, maximum - 1, maximum, maximum + 1, maximum * 2] {
        // Only the length varies: the id is a run of one letter, which the modelled
        // `UserId` pattern accepts at any length.
        let id = "u".repeat(length);
        let case = format!("a {length}-character id");
        let created = create_user(&emulator, &id, "on ~* +@all").await;
        // An id a create stored must be an id every later call can ADDRESS, so each
        // accepted row is read back through `DescribeUsers` before it is deleted.
        let described = emulator
            .call("DescribeUsers", &params(&[("UserId", &id)]))
            .await;
        let deleted = emulator.call("DeleteUser", &params(&[("UserId", &id)])).await;
        if length <= maximum {
            assert_eq!(
                created.status, 200,
                "case {case}: an id at or under the bound must be accepted, got {} {}",
                created.status, created.body
            );
            assert_eq!(
                (described.status, described.text("UserId")),
                (200, Some(id.as_str())),
                "case {case}: DescribeUsers must report the user it created, got {} {}",
                described.status,
                described.body
            );
            assert_eq!(
                deleted.status, 200,
                "case {case}: the user it created must be deletable, got {} {}",
                deleted.status, deleted.body
            );
            continue;
        }
        for (operation, refused) in [("CreateUser", &created), ("DeleteUser", &deleted)] {
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "InvalidParameterValue"),
                "case {case}: {operation} must refuse it with 400 InvalidParameterValue, got {} {}",
                refused.status,
                refused.body
            );
        }
    }
}

/// A deleted user leaves the groups that listed it, and the built-in user a group holds
/// cannot be deleted at all.
#[tokio::test]
async fn a_deleted_user_leaves_the_groups_that_listed_it() {
    let emulator = Emulator::new();
    // Two members, so the group keeps the one that stays: a group must contain the
    // built-in `default` user, which is also the user that can never be deleted.
    for id in ["member-one", "member-two"] {
        let created = create_user(&emulator, id, "on ~* +@all").await;
        assert_eq!(
            created.status, 200,
            "case {id}: CreateUser must answer 200, got {} {}",
            created.status, created.body
        );
    }
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "rbac-group"),
                ("Engine", "valkey"),
                ("UserIds.member.1", state::DEFAULT_USER_ID),
                ("UserIds.member.2", "member-one"),
                ("UserIds.member.3", "member-two"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the group must be created, got {} {}",
        group.status, group.body
    );
    // The membership is the group's, and the user reports it.
    let before = emulator
        .call("DescribeUsers", &params(&[("UserId", "member-one")]))
        .await;
    assert_eq!(
        before.all("UserGroupIds/member"),
        vec!["rbac-group"],
        "the user must report the group it is in, got {}",
        before.body
    );

    // The delete withdraws the memberships, so no later read of the group lists the
    // user. (The delete's own `UserGroupIds` is not evidence of that: a user record
    // carries no memberships, so the list it reports is empty either way.)
    let deleted = emulator
        .call("DeleteUser", &params(&[("UserId", "member-one")]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must answer 200, got {} {}",
        deleted.status, deleted.body
    );
    let group_after = emulator
        .call("DescribeUserGroups", &params(&[("UserGroupId", "rbac-group")]))
        .await;
    assert_eq!(
        group_after.all("UserIds/member"),
        vec![state::DEFAULT_USER_ID, "member-two"],
        "the group must no longer list the deleted user, got {}",
        group_after.body
    );
    // The member that stayed still reports the group, so the withdrawal took only the
    // one membership.
    let survivor = emulator
        .call("DescribeUsers", &params(&[("UserId", "member-two")]))
        .await;
    assert_eq!(
        survivor.all("UserGroupIds/member"),
        vec!["rbac-group"],
        "the remaining member must still report the group, got {}",
        survivor.body
    );

    // The built-in user a group holds is the modelled "default user assigned to the user
    // group" fault, not a not-found and not a success.
    let default_user = emulator
        .call(
            "DeleteUser",
            &params(&[("UserId", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        (default_user.status, default_user.error_code()),
        (400, "DefaultUserAssociatedToUserGroup"),
        "deleting the default user a group holds must be the modelled fault, got {} {}",
        default_user.status,
        default_user.body
    );
}

/// True when `id` has the 8-4-4-4-12 lowercase-hex form of the request id every AWS
/// response reports.
fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && id == id.to_ascii_lowercase()
}

/// The `<RequestId>` child of the `<ErrorResponse>` envelope, asserted to be there and
/// to be a request id.
fn envelope_request_id(case: &str, reply: &Reply) -> String {
    let id = reply
        .text("ErrorResponse/RequestId")
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the error envelope carries no <RequestId> child:\n{}",
                reply.body
            )
        })
        .to_string();
    assert!(
        is_request_id(&id),
        "case {case}: the envelope's request id must be an 8-4-4-4-12 hex id, got {id:?}"
    );
    id
}

/// One error row: the case name, the `Action`, the form pairs that make the request, and
/// the HTTP status plus awsQuery error code it must be answered with (`""` where the
/// layer, not this service, words the code).
type ErrorCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn every_error_envelope_carries_a_request_id() {
    let emulator = Emulator::new();
    // The seeded user the duplicate rows collide with.
    create_user(&emulator, "envelope-user", "on ~* +@all").await;

    // One row per LAYER an error can come from, so the element is not tied to one
    // handler: a handler-returned modelled fault (400 and 404, on three operations), a
    // framework refusal raised before any handler ran (a required member the request
    // omits), and a routing failure (an `Action` this service's model does not bind).
    // `code` is the awsQuery error code where the model fixes it, and `""` where the
    // layer — not this service — words it.
    let cases: &[ErrorCase] = &[
        (
            "CreateUser, duplicate id",
            "CreateUser",
            &[
                ("UserId", "envelope-user"),
                ("UserName", "other-name"),
                ("Engine", "valkey"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "UserAlreadyExists",
        ),
        (
            "CreateUser, an access string that is not an ACL rule list",
            "CreateUser",
            &[
                ("UserId", "rejected-user"),
                ("UserName", "rejected-user"),
                ("Engine", "valkey"),
                ("AccessString", "this is not an acl"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "DescribeUsers, a user that does not exist",
            "DescribeUsers",
            &[("UserId", "absent-user")],
            404,
            "UserNotFound",
        ),
        (
            "DeleteUser, a user that does not exist",
            "DeleteUser",
            &[("UserId", "absent-user")],
            404,
            "UserNotFound",
        ),
        (
            "DescribeCacheClusters, a cluster that does not exist",
            "DescribeCacheClusters",
            &[("CacheClusterId", "absent-cluster")],
            404,
            "CacheClusterNotFound",
        ),
        (
            "CreateUser with no UserId: refused before the handler runs",
            "CreateUser",
            &[
                ("UserName", "nameless"),
                ("Engine", "valkey"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "",
        ),
        (
            "an Action this service's model does not bind",
            "NotAnElastiCacheAction",
            &[],
            400,
            "",
        ),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, action, request, status, code) in cases {
        let reply = emulator.call(action, &params(request)).await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        if !code.is_empty() {
            assert_eq!(
                reply.error_code(),
                *code,
                "case {case}: expected error code {code}, got {:?} in {}",
                reply.error_code(),
                reply.body
            );
        } else {
            assert!(
                !reply.error_code().is_empty(),
                "case {case}: the envelope must still carry an error code, got {}",
                reply.body
            );
        }
        let id = envelope_request_id(case, &reply);
        // A correlation id is per RESPONSE: two errors never report the same one.
        assert!(
            !seen.contains(&id),
            "case {case}: the request id {id:?} was already reported by an earlier response"
        );
        seen.push(id);
    }

    // The same error asked for twice reports two ids, so the element is minted per
    // response rather than per request shape.
    let (first, second) = (
        emulator
            .call("DescribeUsers", &params(&[("UserId", "absent-user")]))
            .await,
        emulator
            .call("DescribeUsers", &params(&[("UserId", "absent-user")]))
            .await,
    );
    assert_ne!(
        envelope_request_id("repeated not-found", &first),
        envelope_request_id("repeated not-found", &second),
        "two responses must report two request ids"
    );

    // The success path is untouched: its request id stays the one element the framework
    // writes, in `ResponseMetadata`, and the envelope rewriter adds nothing to it.
    let ok = emulator
        .call("DescribeUsers", &params(&[("UserId", "envelope-user")]))
        .await;
    assert_eq!(
        ok.status, 200,
        "the success row must answer 200, got {} {}",
        ok.status, ok.body
    );
    assert_eq!(
        ok.body.matches("<RequestId>").count(),
        1,
        "a success must carry exactly one <RequestId>, got {}",
        ok.body
    );
    let success_id = ok
        .text("ResponseMetadata/RequestId")
        .unwrap_or_else(|| panic!("the success reports no ResponseMetadata/RequestId:\n{}", ok.body));
    assert!(
        is_request_id(success_id),
        "the success's request id must be an 8-4-4-4-12 hex id, got {success_id:?}"
    );
}
