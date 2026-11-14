// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyUser`: the change it makes to a stored RBAC user, the
//! members it leaves alone, the transient status it leaves the user in, and the requests
//! it refuses.
//!
//! Every assertion goes over the wire, through the same router the production emulator
//! mounts, and nothing reaches into the service's state struct — so a response that is
//! not computed from persisted state cannot pass. Each accepted row is also read back
//! through `DescribeUsers`, so a change the answer reports but the store never took
//! cannot pass either.
//!
//! The tests are tables. The accepted rows vary along the axes the rule generalises over
//! — which member of the request the change names (the whole access string, rules
//! appended to the one the user has, the engine, each of the three members that say how
//! the user authenticates), how many of them one call names, and what the user looked
//! like before — so no single recorded request is what makes them pass. The refused rows
//! vary along the reason a request cannot be served.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// A password that satisfies the published length rule (`state::PASSWORD_MIN_LENGTH` to
/// `state::PASSWORD_MAX_LENGTH` characters).
const PASSWORD: &str = "aVeryLongPassword1";
/// A second one, for the two-password row.
const PASSWORD_TWO: &str = "aVeryLongPassword2";

/// The `AccessString` every row's user is created with, and the EXPANDED form the service
/// stores for it — the rule list with its command baseline stated outright, which is what
/// an append then adds to.
const CREATED_ACCESS_STRING: &str = "on ~app:* +@read";
const STORED_ACCESS_STRING: &str = "on ~app:* -@all +@read";

/// One accepted row: the case name, the members the `CreateUser` adds to the shared ones,
/// the members the `ModifyUser` names, and the `(element, text)` pairs the modified user
/// must report.
type ModifyCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static [(&'static str, &'static str)],
    &'static [(&'static str, &'static str)],
);

/// The modification table.
///
/// Each row names a different part of the user: `AccessString` states the whole rule list
/// ("Access permissions string used for this user"), `AppendAccessString` adds to the one
/// the user has ("Adds additional user permissions to the access string"), `Engine`
/// switches the engine the user is listed for, and `Passwords` /  `NoPasswordRequired` /
/// `AuthenticationMode` each say how it authenticates. The last rows name two parts at
/// once, so the rule is not tied to a request that changes one thing.
const MODIFY_CASES: &[ModifyCase] = &[
    (
        "the whole access string is replaced",
        &[("NoPasswordRequired", "true")],
        &[("AccessString", "on ~* &* +@all")],
        &[
            ("AccessString", "on ~* &* +@all"),
            ("Engine", "redis"),
            ("Authentication/Type", "no-password"),
        ],
    ),
    (
        "a replacement that states no baseline is stored expanded",
        &[("NoPasswordRequired", "true")],
        &[("AccessString", "on ~metrics:* +@read +@write")],
        &[("AccessString", "on ~metrics:* -@all +@read +@write")],
    ),
    (
        "a category grant is appended to the rules the user has",
        &[("NoPasswordRequired", "true")],
        &[("AppendAccessString", "+@write")],
        &[("AccessString", "on ~app:* -@all +@read +@write")],
    ),
    (
        "a key pattern and a channel pattern are appended",
        &[("NoPasswordRequired", "true")],
        &[("AppendAccessString", "~reports:* &news:*")],
        &[("AccessString", "on ~app:* -@all +@read ~reports:* &news:*")],
    ),
    (
        "a selector is appended as one rule of its own",
        &[("NoPasswordRequired", "true")],
        &[("AppendAccessString", "(+get ~key1)")],
        &[("AccessString", "on ~app:* -@all +@read (+get ~key1)")],
    ),
    (
        "the engine is switched, and nothing else changes",
        &[("NoPasswordRequired", "true")],
        &[("Engine", "valkey")],
        &[
            ("Engine", "valkey"),
            ("AccessString", STORED_ACCESS_STRING),
            ("Authentication/Type", "no-password"),
        ],
    ),
    (
        "a no-password user is given one password through AuthenticationMode",
        &[("NoPasswordRequired", "true")],
        &[
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD),
        ],
        &[
            ("Authentication/Type", "password"),
            ("Authentication/PasswordCount", "1"),
            ("MinimumEngineVersion", "6.0"),
            ("AccessString", STORED_ACCESS_STRING),
        ],
    ),
    (
        "a password user is given two passwords through the older member",
        &[
            ("Passwords.member.1", PASSWORD),
        ],
        &[
            ("Passwords.member.1", PASSWORD),
            ("Passwords.member.2", PASSWORD_TWO),
        ],
        &[
            ("Authentication/Type", "password"),
            ("Authentication/PasswordCount", "2"),
        ],
    ),
    (
        "a password user drops its passwords",
        &[("Passwords.member.1", PASSWORD)],
        &[("NoPasswordRequired", "true")],
        &[("Authentication/Type", "no-password")],
    ),
    (
        "a password user switches to IAM, which needs the later engine version",
        &[("Passwords.member.1", PASSWORD)],
        &[("AuthenticationMode.Type", "iam")],
        &[
            ("Authentication/Type", "iam"),
            ("MinimumEngineVersion", "7.0"),
        ],
    ),
    (
        "the access string and the password mode in one call",
        &[("NoPasswordRequired", "true")],
        &[
            ("AccessString", "on ~orders:* +@read"),
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD),
        ],
        &[
            ("AccessString", "on ~orders:* -@all +@read"),
            ("Authentication/Type", "password"),
            ("Authentication/PasswordCount", "1"),
        ],
    ),
    (
        "appended rules and the engine in one call",
        &[("NoPasswordRequired", "true")],
        &[("AppendAccessString", "-@admin"), ("Engine", "valkey")],
        &[
            ("AccessString", "on ~app:* -@all +@read -@admin"),
            ("Engine", "valkey"),
        ],
    ),
];

/// Create the user a row starts from: the shared members, plus the row's own.
async fn create_user(
    emulator: &Emulator,
    case: &str,
    id: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut pairs = params(&[
        ("UserId", id),
        ("UserName", id),
        ("Engine", "redis"),
        ("AccessString", CREATED_ACCESS_STRING),
    ]);
    pairs.extend(params(extra));
    let created = emulator.call("CreateUser", &pairs).await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUser must answer 200, got {} {}",
        created.status, created.body
    );
    assert_eq!(
        created.text("AccessString"),
        Some(STORED_ACCESS_STRING),
        "case {case}: the user must start from {STORED_ACCESS_STRING:?}, got {:?}",
        created.text("AccessString")
    );
    created
}

/// Read one user back by id, asserting the describe itself succeeded.
///
/// `pub(crate)` so the suite about a request that states how a user authenticates twice
/// reads its users back the same way rather than carrying a second copy of this.
pub(crate) async fn describe_user(emulator: &Emulator, case: &str, id: &str) -> Reply {
    let described = emulator
        .call("DescribeUsers", &params(&[("UserId", id)]))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeUsers must answer 200, got {} {}",
        described.status, described.body
    );
    described
}

/// Assert `reply` reports every `(element, text)` pair of `expected`.
fn assert_reports(case: &str, what: &str, reply: &Reply, expected: &[(&str, &str)]) {
    for (element, text) in expected {
        assert_eq!(
            reply.text(element),
            Some(*text),
            "case {case}: {what} must report {element} {text:?}, got {:?} in {}",
            reply.text(element),
            reply.body
        );
    }
}

#[tokio::test]
async fn a_modify_changes_the_members_it_names_and_leaves_the_rest() {
    // The wait below must out-last the span the service publishes, or a test would read a
    // user the clock had not settled yet and report it as a failure of the handler.
    assert!(
        PAST_TRANSITION > state::CLUSTER_TRANSITION,
        "these tests wait {PAST_TRANSITION:?}, which must out-last the span ({:?}) this \
         service publishes for a status change",
        state::CLUSTER_TRANSITION
    );
    let emulator = Emulator::new();
    for (index, (case, created_with, modification, expected)) in MODIFY_CASES.iter().enumerate() {
        // One user per row, so each row reads back its own stored user.
        let id = format!("modified-user-{index}");
        let created = create_user(&emulator, case, &id, created_with).await;

        let mut request = params(&[("UserId", &id)]);
        request.extend(params(modification));
        let modified = emulator.call("ModifyUser", &request).await;
        assert_eq!(
            modified.status, 200,
            "case {case}: ModifyUser must answer 200, got {} {}",
            modified.status, modified.body
        );
        // The change the request asked for is in the answer, which is the stored user.
        assert_reports(case, "ModifyUser", &modified, expected);
        // A user ElastiCache is applying a change to is not settled: it reports the one
        // status that says so, and the clock owes it `active`.
        assert_eq!(
            modified.text("Status"),
            Some(state::STATUS_MODIFYING),
            "case {case}: ModifyUser must report the user {:?}, got {:?}",
            state::STATUS_MODIFYING,
            modified.text("Status")
        );
        // The identity of the user is untouched: a modify changes what the user may do,
        // never which user it is.
        for member in ["UserId", "UserName", "ARN"] {
            assert_eq!(
                modified.text(member),
                created.text(member),
                "case {case}: ModifyUser must leave {member} as the create stored it ({:?}), \
                 got {:?}",
                created.text(member),
                modified.text(member)
            );
        }

        // The read that follows is answered once the status the clock owed has been
        // materialised, and the change is the one the store holds.
        let settled = describe_user(&emulator, case, &id).await;
        assert_eq!(
            settled.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {case}: the read after the modify must report {:?}, got {}",
            state::USER_STATUS_ACTIVE,
            settled.body
        );
        assert_reports(case, "DescribeUsers after the modify", &settled, expected);
        for member in ["UserId", "UserName", "ARN"] {
            assert_eq!(
                settled.text(member),
                created.text(member),
                "case {case}: the stored user must keep {member} ({:?}), got {:?}",
                created.text(member),
                settled.text(member)
            );
        }
    }
}

/// A modify leaves the user's group memberships alone, and the settled user takes the
/// next change: the two things a change-and-settle cycle must not break.
#[tokio::test]
async fn a_modified_user_keeps_its_groups_and_takes_the_next_change() {
    let emulator = Emulator::new();
    create_user(&emulator, "grouped", "grouped-user", &[("NoPasswordRequired", "true")]).await;
    let group = emulator
        .call(
            "CreateUserGroup",
            &params(&[
                ("UserGroupId", "modify-group"),
                ("Engine", "redis"),
                ("UserIds.member.1", state::DEFAULT_USER_ID),
                ("UserIds.member.2", "grouped-user"),
            ]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the group must be created, got {} {}",
        group.status, group.body
    );

    let modified = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "grouped-user"), ("AppendAccessString", "+@write")]),
        )
        .await;
    assert_eq!(
        modified.status, 200,
        "the modify must answer 200, got {} {}",
        modified.status, modified.body
    );
    assert_eq!(
        modified.all("UserGroupIds/member"),
        vec!["modify-group"],
        "the modified user must still report the group it is in, got {}",
        modified.body
    );
    let group_after = emulator
        .call(
            "DescribeUserGroups",
            &params(&[("UserGroupId", "modify-group")]),
        )
        .await;
    assert_eq!(
        group_after.all("UserIds/member"),
        vec![state::DEFAULT_USER_ID, "grouped-user"],
        "the group must still list the modified user, got {}",
        group_after.body
    );

    // Once the change has settled the user takes the next one, which appends to the rules
    // the first change left — so an append composes with the one before it.
    std::thread::sleep(PAST_TRANSITION);
    let again = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "grouped-user"), ("AppendAccessString", "~audit:*")]),
        )
        .await;
    assert_eq!(
        (again.status, again.text("AccessString")),
        (200, Some("on ~app:* -@all +@read +@write ~audit:*")),
        "a settled user must take the next append, got {} {}",
        again.status,
        again.body
    );

    // The built-in user every account has is untouched by all of that.
    let default_user = emulator
        .call(
            "DescribeUsers",
            &params(&[("UserId", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        (
            default_user.text("Status"),
            default_user.text("AccessString")
        ),
        (
            Some(state::USER_STATUS_ACTIVE),
            Some(state::DEFAULT_USER_ACCESS_STRING)
        ),
        "the built-in user must be unchanged, got {}",
        default_user.body
    );
}

/// The stored rule list is the one thing a caller can grow from call to call, so the
/// append path is bounded: a result at the maximum the service keeps is accepted and one
/// past it is refused. The bound is `state::MAX_ACCESS_STRING_LEN`.
#[tokio::test]
async fn an_append_past_the_access_string_bound_is_refused() {
    let emulator = Emulator::new();
    // Both rows start from the same stored list, so the room each append has is the same.
    let room = state::MAX_ACCESS_STRING_LEN - STORED_ACCESS_STRING.chars().count() - 1;
    // Each row's appended rules are ONE key pattern, whose glob carries the length: the
    // axis here is the length of the result, not the rules it is made of.
    for (case, id, appended_len, status, code) in [
        ("a result exactly at the bound", "bounded-user", room, 200, ""),
        (
            "a result one character past the bound",
            "unbounded-user",
            room + 1,
            400,
            "InvalidParameterValue",
        ),
    ] {
        create_user(&emulator, case, id, &[("NoPasswordRequired", "true")]).await;
        let appended = format!("~{}", "k".repeat(appended_len - 1));
        let reply = emulator
            .call(
                "ModifyUser",
                &params(&[("UserId", id), ("AppendAccessString", &appended)]),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (status, code),
            "case {case}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        let stored = describe_user(&emulator, case, id).await;
        let expected = if status == 200 {
            format!("{STORED_ACCESS_STRING} {appended}")
        } else {
            STORED_ACCESS_STRING.to_string()
        };
        assert_eq!(
            stored.text("AccessString"),
            Some(expected.as_str()),
            "case {case}: the stored rule list must be {} characters, got {:?}",
            expected.chars().count(),
            stored.text("AccessString").map(str::len)
        );
    }
}

/// One refused row: the case name, the `ModifyUser` members, and the HTTP status plus
/// awsQuery error code it must be answered with.
type RefusalCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

/// The refusal table: one row per reason a modify cannot be served, over the user
/// `a_modify_the_service_refuses` seeds.
const REFUSAL_CASES: &[RefusalCase] = &[
    (
        "a user that does not exist",
        &[("UserId", "absent-user"), ("AccessString", "on ~* +@all")],
        404,
        "UserNotFound",
    ),
    (
        "an access string that is not an ACL rule list",
        &[
            ("UserId", "refusal-user"),
            ("AccessString", "this is not an acl"),
        ],
        400,
        "InvalidParameterValue",
    ),
    (
        "appended rules that are not ACL rules",
        &[
            ("UserId", "refusal-user"),
            ("AppendAccessString", "grant-everything"),
        ],
        400,
        "InvalidParameterValue",
    ),
    (
        "a selector that is never closed",
        &[
            ("UserId", "refusal-user"),
            ("AppendAccessString", "(+get ~key1"),
        ],
        400,
        "InvalidParameterValue",
    ),
    (
        "an engine access control does not exist for",
        &[("UserId", "refusal-user"), ("Engine", "memcached")],
        400,
        "InvalidParameterValue",
    ),
    (
        "stating the access string and appending to it at once",
        &[
            ("UserId", "refusal-user"),
            ("AccessString", "on ~* +@all"),
            ("AppendAccessString", "+@write"),
        ],
        400,
        "InvalidParameterCombination",
    ),
    (
        "no modification at all",
        &[("UserId", "refusal-user")],
        400,
        "InvalidParameterCombination",
    ),
    (
        "two members that each say how the user authenticates",
        &[
            ("UserId", "refusal-user"),
            ("Passwords.member.1", PASSWORD),
            ("AuthenticationMode.Type", "no-password-required"),
        ],
        400,
        "InvalidParameterCombination",
    ),
    (
        "an IAM mode that also carries passwords",
        &[
            ("UserId", "refusal-user"),
            ("AuthenticationMode.Type", "iam"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD),
        ],
        400,
        "InvalidParameterCombination",
    ),
    (
        "a password shorter than the published minimum",
        &[("UserId", "refusal-user"), ("Passwords.member.1", "short")],
        400,
        "InvalidParameterValue",
    ),
    (
        "more passwords than a user may hold",
        &[
            ("UserId", "refusal-user"),
            ("Passwords.member.1", PASSWORD),
            ("Passwords.member.2", PASSWORD_TWO),
            ("Passwords.member.3", "aVeryLongPassword3"),
        ],
        400,
        "InvalidParameterValue",
    ),
];

#[tokio::test]
async fn a_modify_the_service_refuses() {
    let emulator = Emulator::new();
    create_user(
        &emulator,
        "refusal",
        "refusal-user",
        &[("NoPasswordRequired", "true")],
    )
    .await;
    for (case, request, status, code) in REFUSAL_CASES {
        let refused = emulator.call("ModifyUser", &params(request)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        // A refused request changes nothing: the user still has the rule list the create
        // stored.
        let unchanged = describe_user(&emulator, case, "refusal-user").await;
        assert_eq!(
            (unchanged.text("AccessString"), unchanged.text("Status")),
            (
                Some(STORED_ACCESS_STRING),
                Some(state::USER_STATUS_ACTIVE)
            ),
            "case {case}: a refused modify must leave the user as it was, got {}",
            unchanged.body
        );
    }

    // An id longer than the maximum the service publishes names no user it could have
    // stored, so it is refused for what is wrong with the id.
    let long_id = "u".repeat(state::MAX_USER_ID_LEN + 1);
    let refused = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", &long_id), ("AccessString", "on ~* +@all")]),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterValue"),
        "an id past the length bound must be refused, got {} {}",
        refused.status,
        refused.body
    );

    // A user still carrying a change the clock has not settled is not in active state,
    // which is the modelled state fault — and so is one on its way out.
    let first = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "refusal-user"), ("Engine", "valkey")]),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first modify must answer 200, got {} {}",
        first.status, first.body
    );
    let second = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "refusal-user"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        (second.status, second.error_code()),
        (400, "InvalidUserState"),
        "modifying a user that is already modifying must be the modelled state fault, got {} {}",
        second.status,
        second.body
    );
    // The refused second call did not take effect: the engine is the one the first call
    // asked for.
    std::thread::sleep(PAST_TRANSITION);
    let settled = describe_user(&emulator, "state gate", "refusal-user").await;
    assert_eq!(
        (settled.text("Engine"), settled.text("Status")),
        (Some("valkey"), Some(state::USER_STATUS_ACTIVE)),
        "the settled user must carry the first change only, got {}",
        settled.body
    );
    let deleted = emulator
        .call("DeleteUser", &params(&[("UserId", "refusal-user")]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "the settled user must be deletable, got {} {}",
        deleted.status, deleted.body
    );
    let modified_while_deleting = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "refusal-user"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        (
            modified_while_deleting.status,
            modified_while_deleting.error_code()
        ),
        (400, "InvalidUserState"),
        "modifying a user on its way out must be the modelled state fault, got {} {}",
        modified_while_deleting.status,
        modified_while_deleting.body
    );
    // Once the removal falls due the user is gone, so the modify is a not-found.
    std::thread::sleep(PAST_TRANSITION);
    let gone = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", "refusal-user"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code()),
        (404, "UserNotFound"),
        "modifying a user that is gone must be the modelled not-found, got {} {}",
        gone.status,
        gone.body
    );
}
