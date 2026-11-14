// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ElastiCache RBAC user: `CreateUser` and the
//! `DescribeUsers` that reads one back.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and nothing reaches into the service's state struct — so a
//! response that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the ones the `cache_cluster_lifecycle` suite
//! marks `pub(crate)`, so this target keeps one harness rather than a second
//! copy that could drift.
//!
//! The tests are tables: each row is one input along the axis the rule under
//! test generalises over (the engine and its spelling, the id's letter case,
//! which of the three authentication members says how the user authenticates,
//! the ACL rule forms an access string carries, the page size, the property
//! filtered on), so no single recorded request is what makes them pass.

use std::collections::BTreeSet;
use std::sync::Arc;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The region these tests sign with; it is the region the user ARNs must name.
const REGION: &str = "us-east-1";

/// A password long enough to be a realistic one. The model puts no constraint on
/// `PasswordListInput`'s members, so its only role here is to be a password.
const PASSWORD: &str = "aVeryLongPassword1";
/// A second one, for the two-password rows.
const PASSWORD_TWO: &str = "aVeryLongPassword2";

/// One row of the create/round-trip table: the request, and the user the service
/// must store and report for it.
#[derive(Debug, Clone)]
struct UserCase {
    case: &'static str,
    /// The form pairs that make the `CreateUser` request.
    request: &'static [(&'static str, &'static str)],
    /// The identity the user is stored and reported under.
    stored_id: &'static str,
    user_name: &'static str,
    engine: &'static str,
    access_string: &'static str,
    /// `Authentication$Type` — an `AuthenticationType` token.
    authentication_type: &'static str,
    /// `Authentication$PasswordCount`, or `None` when the user holds none.
    password_count: Option<&'static str>,
    minimum_engine_version: &'static str,
}

/// Every `User` member of one reply, keyed the way a client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireUser {
    user_id: String,
    user_name: String,
    status: String,
    engine: String,
    minimum_engine_version: String,
    access_string: String,
    authentication_type: String,
    password_count: Option<String>,
    arn: String,
}

impl WireUser {
    /// The single user a reply reports.
    fn read(case: &str, reply: &Reply) -> Self {
        let text = |suffix: &str| {
            reply
                .text(suffix)
                .unwrap_or_else(|| {
                    panic!("case {case}: the reply reports no {suffix}:\n{}", reply.body)
                })
                .to_string()
        };
        Self {
            user_id: text("UserId"),
            user_name: text("UserName"),
            status: text("Status"),
            engine: text("Engine"),
            minimum_engine_version: text("MinimumEngineVersion"),
            access_string: text("AccessString"),
            authentication_type: text("Authentication/Type"),
            password_count: reply
                .text("Authentication/PasswordCount")
                .map(str::to_string),
            arn: text("ARN"),
        }
    }
}

/// The create/round-trip table.
///
/// The axes: the engine RBAC supports and the spelling it is sent in
/// (`CreateUser$Engine`: "The options are valkey or redis"), the letter case of
/// the id ("This value is stored as a lowercase string"), which of the three
/// authentication members says how the user authenticates (`Passwords`,
/// `NoPasswordRequired`, `AuthenticationMode`) and how many passwords it names,
/// and the ACL rule forms the access string carries.
fn create_cases() -> [UserCase; 9] {
    [
        UserCase {
            case: "redis, NoPasswordRequired",
            request: &[
                ("UserId", "demo-user"),
                ("UserName", "demo-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            stored_id: "demo-user",
            user_name: "demo-user",
            engine: "redis",
            access_string: "on ~* +@all",
            authentication_type: "no-password",
            password_count: None,
            minimum_engine_version: "6.0",
        },
        UserCase {
            case: "valkey, one password, mixed-case id",
            request: &[
                ("UserId", "App-Reader"),
                ("UserName", "app-reader"),
                ("Engine", "valkey"),
                ("AccessString", "on ~app:* -@all +@read"),
                ("Passwords.member.1", PASSWORD),
            ],
            // "This value is stored as a lowercase string".
            stored_id: "app-reader",
            user_name: "app-reader",
            engine: "valkey",
            access_string: "on ~app:* -@all +@read",
            authentication_type: "password",
            password_count: Some("1"),
            // Valkey's first ElastiCache line is 7.2, so a user listed for it
            // cannot run on an older engine version.
            minimum_engine_version: "7.2",
        },
        UserCase {
            case: "two passwords",
            request: &[
                ("UserId", "two-password-user"),
                ("UserName", "two-password-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("Passwords.member.1", PASSWORD),
                ("Passwords.member.2", PASSWORD_TWO),
            ],
            stored_id: "two-password-user",
            user_name: "two-password-user",
            engine: "redis",
            access_string: "on ~* +@all",
            authentication_type: "password",
            password_count: Some("2"),
            minimum_engine_version: "6.0",
        },
        UserCase {
            case: "AuthenticationMode password",
            request: &[
                ("UserId", "mode-password-user"),
                ("UserName", "mode-password-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("AuthenticationMode.Type", "password"),
                ("AuthenticationMode.Passwords.member.1", PASSWORD),
            ],
            stored_id: "mode-password-user",
            user_name: "mode-password-user",
            engine: "redis",
            access_string: "on ~* +@all",
            authentication_type: "password",
            password_count: Some("1"),
            minimum_engine_version: "6.0",
        },
        UserCase {
            case: "AuthenticationMode no-password-required",
            request: &[
                ("UserId", "mode-nopassword-user"),
                ("UserName", "mode-nopassword-user"),
                ("Engine", "valkey"),
                ("AccessString", "off ~* +@all"),
                ("AuthenticationMode.Type", "no-password-required"),
            ],
            stored_id: "mode-nopassword-user",
            user_name: "mode-nopassword-user",
            engine: "valkey",
            access_string: "off ~* +@all",
            // `no-password-required` is what a request asks for; `no-password` is
            // what the user reports back (`AuthenticationType`).
            authentication_type: "no-password",
            password_count: None,
            minimum_engine_version: "7.2",
        },
        UserCase {
            case: "AuthenticationMode iam",
            request: &[
                ("UserId", "mode-iam-user"),
                ("UserName", "mode-iam-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("AuthenticationMode.Type", "iam"),
            ],
            stored_id: "mode-iam-user",
            user_name: "mode-iam-user",
            engine: "redis",
            access_string: "on ~* +@all",
            authentication_type: "iam",
            password_count: None,
            // IAM authentication for an RBAC user arrived in Redis OSS 7.0, so a
            // user that authenticates that way reports that minimum.
            minimum_engine_version: "7.0",
        },
        UserCase {
            case: "mixed-case engine and whitespace-separated rules",
            request: &[
                ("UserId", "mixed-engine-user"),
                ("UserName", "mixed-engine-user"),
                ("Engine", "ReDiS"),
                (
                    "AccessString",
                    "  on   %RW~stats:*  &news:*  resetchannels +get +config|get  ",
                ),
                ("NoPasswordRequired", "true"),
            ],
            stored_id: "mixed-engine-user",
            user_name: "mixed-engine-user",
            engine: "redis",
            // Only the whitespace between rules is normalised; no rule is added,
            // removed or reordered — except the command baseline the service
            // expands the list with, which this list does not state (see the
            // normalization table in `rbac_user_access_string_and_deletion.rs`).
            access_string: "on %RW~stats:* &news:* resetchannels -@all +get +config|get",
            authentication_type: "no-password",
            password_count: None,
            minimum_engine_version: "6.0",
        },
        UserCase {
            case: "tags on the create",
            request: &[
                ("UserId", "tagged-user"),
                ("UserName", "tagged-user"),
                ("Engine", "valkey"),
                ("AccessString", "on ~* &* +@all nopass"),
                ("NoPasswordRequired", "true"),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "team"),
            ],
            stored_id: "tagged-user",
            user_name: "tagged-user",
            engine: "valkey",
            access_string: "on ~* &* +@all nopass",
            authentication_type: "no-password",
            password_count: None,
            minimum_engine_version: "7.2",
        },
        UserCase {
            case: "a selector and the selector keyword",
            request: &[
                ("UserId", "selector-user"),
                ("UserName", "selector-user"),
                ("Engine", "valkey"),
                (
                    "AccessString",
                    "on ~* resetchannels -@all clearselectors (+get ~key1)",
                ),
                ("NoPasswordRequired", "true"),
            ],
            stored_id: "selector-user",
            user_name: "selector-user",
            engine: "valkey",
            // A selector is one rule carrying a rule list of its own, and it
            // survives the round trip as written.
            access_string: "on ~* resetchannels -@all clearselectors (+get ~key1)",
            authentication_type: "no-password",
            password_count: None,
            minimum_engine_version: "7.2",
        },
    ]
}

#[tokio::test]
async fn a_created_user_is_read_back_from_persisted_state() {
    let emulator = Emulator::new();
    for case in create_cases() {
        let created = emulator.call("CreateUser", &params(case.request)).await;
        assert_eq!(
            created.status, 200,
            "case {}: create must succeed, got {}",
            case.case, created.body
        );
        let reported = WireUser::read(case.case, &created);
        let expected = WireUser {
            user_id: case.stored_id.to_string(),
            user_name: case.user_name.to_string(),
            // A user ElastiCache has accepted is usable at once.
            status: state::USER_STATUS_ACTIVE.to_string(),
            engine: case.engine.to_string(),
            minimum_engine_version: case.minimum_engine_version.to_string(),
            access_string: case.access_string.to_string(),
            authentication_type: case.authentication_type.to_string(),
            password_count: case.password_count.map(str::to_string),
            arn: reported.arn.clone(),
        };
        assert_eq!(
            reported, expected,
            "case {}: the created user must be the one the request describes, got {reported:?}",
            case.case
        );
        // The ARN names this service, the request's region and the stored id.
        let prefix = format!("arn:aws:elasticache:{REGION}:");
        assert!(
            reported.arn.starts_with(&prefix)
                && reported.arn.ends_with(&format!(":user:{}", case.stored_id)),
            "case {}: ARN must be {prefix}<account>:user:{}, got {:?}",
            case.case,
            case.stored_id,
            reported.arn
        );

        // The round trip: the same user, read back through `DescribeUsers` out of
        // persisted state. The id is looked up in a DIFFERENT letter case from
        // the one that was stored, because the id is case-folded on both paths.
        let described = emulator
            .call(
                "DescribeUsers",
                &params(&[("UserId", &case.stored_id.to_ascii_uppercase())]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {}: describe must succeed, got {}",
            case.case, described.body
        );
        let read_back = WireUser::read(case.case, &described);
        assert_eq!(
            read_back, reported,
            "case {}: the describe must report the persisted user, got {read_back:?}",
            case.case
        );
        // The user belongs to no user group yet, so the list is empty rather than
        // invented.
        assert!(
            described.all("UserGroupIds").is_empty(),
            "case {}: a user in no group must report no group ids, got {:?}",
            case.case,
            described.all("UserGroupIds")
        );
    }
}

/// One refusal row: the case name, the form pairs, and the HTTP status plus
/// `awsQuery` error code the request must be answered with.
type Refusal = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn create_refuses_a_user_the_service_could_not_serve() {
    let emulator = Emulator::new();
    // One user to collide with.
    let first = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "taken-user"),
                ("UserName", "taken-name"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed: {}",
        first.body
    );

    // The axes: the identity that collides, the engine, the ACL rule that is not
    // one, and each combination of the three authentication members that cannot
    // hold together.
    let cases: [Refusal; 16] = [
        (
            "the same id again",
            &[
                ("UserId", "taken-user"),
                ("UserName", "another-name"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "UserAlreadyExists",
        ),
        (
            "the same id in another letter case",
            &[
                ("UserId", "TAKEN-USER"),
                ("UserName", "another-name"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "UserAlreadyExists",
        ),
        // A user NAME another user already has is NOT a refusal: uniqueness is on
        // `UserId`, and a user group needs a member whose name is `default`, which
        // a caller creates its own user for. See
        // `a_user_name_is_unique_within_a_group_and_not_across_the_account` in
        // `tests/all/cache_membership_rules.rs`.
        (
            // A legal CLUSTER engine, but RBAC is a Valkey / Redis OSS feature.
            "engine memcached",
            &[
                ("UserId", "memcached-user"),
                ("UserName", "memcached-user"),
                ("Engine", "memcached"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an engine no cache runs",
            &[
                ("UserId", "mysql-user"),
                ("UserName", "mysql-user"),
                ("Engine", "mysql"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an access string of bare words",
            &[
                ("UserId", "words-user"),
                ("UserName", "words-user"),
                ("Engine", "redis"),
                ("AccessString", "this is not an acl"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one rule among good ones is not a rule",
            &[
                ("UserId", "mixed-rules-user"),
                ("UserName", "mixed-rules-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* garbage! +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a key pattern with no glob",
            &[
                ("UserId", "empty-pattern-user"),
                ("UserName", "empty-pattern-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~ +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a key qualifier that is not read or write",
            &[
                ("UserId", "bad-qualifier-user"),
                ("UserName", "bad-qualifier-user"),
                ("Engine", "redis"),
                ("AccessString", "on %X~app:* +@all"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a command rule naming no command",
            &[
                ("UserId", "bare-plus-user"),
                ("UserName", "bare-plus-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "more passwords than a user may hold",
            &[
                ("UserId", "three-password-user"),
                ("UserName", "three-password-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("Passwords.member.1", PASSWORD),
                ("Passwords.member.2", PASSWORD_TWO),
                ("Passwords.member.3", "aVeryLongPassword3"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "an authentication mode type the model does not declare",
            &[
                ("UserId", "bad-mode-user"),
                ("UserName", "bad-mode-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("AuthenticationMode.Type", "certificate"),
            ],
            400,
            // `InputAuthenticationType` is a modelled enum, so the boundary
            // validator refuses a token outside it before the handler runs; on
            // the query protocols that refusal is `ValidationError`.
            "ValidationError",
        ),
        (
            "passwords and NoPasswordRequired together",
            &[
                ("UserId", "both-auth-user"),
                ("UserName", "both-auth-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("Passwords.member.1", PASSWORD),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an authentication mode beside the older members",
            &[
                ("UserId", "mode-and-flag-user"),
                ("UserName", "mode-and-flag-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("AuthenticationMode.Type", "no-password-required"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a selector that is never closed",
            &[
                ("UserId", "open-selector-user"),
                ("UserName", "open-selector-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* (+get"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a selector holding something that is not a rule",
            &[
                ("UserId", "bad-selector-user"),
                ("UserName", "bad-selector-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* (+get nonsense!)"),
                ("NoPasswordRequired", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "nothing saying how the user authenticates",
            &[
                ("UserId", "no-auth-user"),
                ("UserName", "no-auth-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
            ],
            400,
            "InvalidParameterCombination",
        ),
    ];
    for (case, pairs, status, code) in cases {
        let reply = emulator.call("CreateUser", &params(pairs)).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected error code {code}, got {:?}",
            reply.body
        );
        // A refused create persists nothing: the id it named is still free.
        let described = emulator
            .call(
                "DescribeUsers",
                &params(&[("UserId", pairs[0].1)]),
            )
            .await;
        let stored_before = pairs[0].1.eq_ignore_ascii_case("taken-user");
        assert_eq!(
            described.status,
            if stored_before { 200 } else { 404 },
            "case {case}: a refused create must leave the id unused, got {}",
            described.body
        );
    }
}

#[tokio::test]
async fn an_authentication_mode_password_with_no_passwords_is_refused() {
    let emulator = Emulator::new();
    // `AuthenticationMode.Passwords` has a modelled minimum length, so the only
    // way to ask for password authentication without one is to omit the member.
    let reply = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "mode-no-passwords"),
                ("UserName", "mode-no-passwords"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("AuthenticationMode.Type", "password"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "a password mode with no passwords must be refused, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterCombination",
        "expected InvalidParameterCombination, got {}",
        reply.body
    );
}

#[tokio::test]
async fn a_tag_with_no_key_is_refused_through_the_modelled_channel() {
    let emulator = Emulator::new();
    let reply = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "keyless-tag-user"),
                ("UserName", "keyless-tag-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
                // A tag that carries only a value: `Tag$Key` may not be null.
                ("Tags.Tag.1.Value", "orphan"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "a tag with no Key must be refused, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterValue",
        "expected InvalidParameterValue, got {}",
        reply.body
    );
}

#[tokio::test]
async fn more_tags_than_a_resource_may_carry_are_refused() {
    let emulator = Emulator::new();
    let mut pairs = params(&[
        ("UserId", "over-tagged-user"),
        ("UserName", "over-tagged-user"),
        ("Engine", "redis"),
        ("AccessString", "on ~* +@all"),
        ("NoPasswordRequired", "true"),
    ]);
    for index in 1..=state::MAX_TAGS_PER_RESOURCE + 1 {
        pairs.push((format!("Tags.Tag.{index}.Key"), format!("k{index}")));
        pairs.push((format!("Tags.Tag.{index}.Value"), format!("v{index}")));
    }
    let reply = emulator.call("CreateUser", &pairs).await;
    assert_eq!(
        reply.status, 400,
        "more than {} tags must be refused, got {}",
        state::MAX_TAGS_PER_RESOURCE,
        reply.body
    );
    assert_eq!(
        reply.error_code(),
        "TagQuotaPerResourceExceeded",
        "expected TagQuotaPerResourceExceeded, got {}",
        reply.body
    );
}

#[tokio::test]
async fn the_store_is_bounded_by_the_user_quota() {
    let emulator = Emulator::new();
    // The built-in `default` user counts against the quota, so the account can
    // create one fewer than it.
    for index in 1..state::USER_QUOTA {
        let id = format!("quota-user-{index}");
        let reply = emulator
            .call(
                "CreateUser",
                &params(&[
                    ("UserId", &id),
                    ("UserName", &id),
                    ("Engine", "redis"),
                    ("AccessString", "on ~* +@all"),
                    ("NoPasswordRequired", "true"),
                ]),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "user {index} of {} must be created, got {}",
            state::USER_QUOTA,
            reply.body
        );
    }
    let over = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "one-too-many"),
                ("UserName", "one-too-many"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        over.error_code(),
        "UserQuotaExceeded",
        "the {}th user must be refused, got {}",
        state::USER_QUOTA + 1,
        over.body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_of_many_concurrent_creates_of_one_id_succeeds() {
    // The same id from several callers at once: the id is the resource's
    // identity, so exactly one call may store a user and the rest must be
    // refused — whatever order they are served in.
    for concurrency in [2_usize, 3, 5] {
        let emulator = Arc::new(Emulator::new());
        let id = format!("race-user-{concurrency}");
        let mut callers = Vec::with_capacity(concurrency);
        for caller in 0..concurrency {
            let emulator = Arc::clone(&emulator);
            let id = id.clone();
            callers.push(tokio::spawn(async move {
                // Each caller sends its own username, so a loser cannot be
                // refused for the username instead of the id.
                let name = format!("{id}-{caller}");
                let reply = emulator
                    .call(
                        "CreateUser",
                        &params(&[
                            ("UserId", &id),
                            ("UserName", &name),
                            ("Engine", "redis"),
                            ("AccessString", "on ~* +@all"),
                            ("NoPasswordRequired", "true"),
                        ]),
                    )
                    .await;
                (reply.status, reply.error_code().to_string())
            }));
        }
        let mut results: Vec<(u16, String)> = Vec::with_capacity(concurrency);
        for caller in callers {
            results.push(caller.await.expect("a create must not panic"));
        }
        let successes = results.iter().filter(|(status, _)| *status == 200).count();
        assert_eq!(
            successes, 1,
            "case {concurrency} callers: expected exactly 1 success, got {successes} from \
             {results:?}"
        );
        for (status, code) in results.iter().filter(|(status, _)| *status != 200) {
            assert_eq!(
                (*status, code.as_str()),
                (400, "UserAlreadyExists"),
                "case {concurrency} callers: a loser must be refused for the id, got \
                 {status} {code}"
            );
        }
    }
}

/// Create one user with the given id and name, and fail the test if it is not
/// created.
async fn create_user(emulator: &Emulator, id: &str, engine: &str) {
    let reply = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", engine),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "user {id} must be created, got {}",
        reply.body
    );
}

#[tokio::test]
async fn describe_reports_every_stored_user_and_the_built_in_one() {
    let emulator = Emulator::new();
    // Before any create, the account already has the one user ElastiCache gives
    // it: `default`, which no call creates.
    let built_in = emulator.call("DescribeUsers", &params(&[])).await;
    assert_eq!(
        built_in.status, 200,
        "describe must succeed on an empty account, got {}",
        built_in.body
    );
    assert_eq!(
        built_in.all("UserId"),
        vec![state::DEFAULT_USER_ID],
        "an account with no created user must report the built-in one, got {}",
        built_in.body
    );

    for (id, engine) in [
        ("alpha-user", "redis"),
        ("beta-user", "valkey"),
        ("gamma-user", "redis"),
        ("delta-user", "valkey"),
    ] {
        create_user(&emulator, id, engine).await;
    }

    let all = emulator.call("DescribeUsers", &params(&[])).await;
    assert_eq!(
        all.all("UserId"),
        vec![
            "alpha-user",
            "beta-user",
            state::DEFAULT_USER_ID,
            "delta-user",
            "gamma-user",
        ],
        "every stored user must be reported, in id order, got {}",
        all.body
    );

    // The engine filter: only the users running it, over both engines.
    for (engine, expected) in [
        ("redis", vec!["alpha-user", state::DEFAULT_USER_ID, "gamma-user"]),
        ("valkey", vec!["beta-user", "delta-user"]),
    ] {
        let reply = emulator
            .call("DescribeUsers", &params(&[("Engine", engine)]))
            .await;
        assert_eq!(
            reply.all("UserId"),
            expected,
            "case engine {engine}: expected {expected:?}, got {}",
            reply.body
        );
    }

    // A `Filter` names a property of the user being reported, so each supported
    // name selects on that property — and one the shape has no member for is
    // refused rather than quietly matching nothing.
    for (case, pairs, expected) in [
        (
            "UserId",
            &[
                ("Filters.member.1.Name", "UserId"),
                ("Filters.member.1.Values.member.1", "beta-user"),
            ][..],
            vec!["beta-user"],
        ),
        (
            "UserName over two values",
            &[
                ("Filters.member.1.Name", "UserName"),
                ("Filters.member.1.Values.member.1", "alpha-user"),
                ("Filters.member.1.Values.member.2", "delta-user"),
            ][..],
            vec!["alpha-user", "delta-user"],
        ),
        (
            "Status",
            &[
                ("Filters.member.1.Name", "Status"),
                ("Filters.member.1.Values.member.1", "deleting"),
            ][..],
            Vec::new(),
        ),
        (
            "two filters hold together",
            &[
                ("Filters.member.1.Name", "Engine"),
                ("Filters.member.1.Values.member.1", "valkey"),
                ("Filters.member.2.Name", "UserName"),
                ("Filters.member.2.Values.member.1", "delta-user"),
            ][..],
            vec!["delta-user"],
        ),
        (
            "MinimumEngineVersion",
            &[
                ("Filters.member.1.Name", "MinimumEngineVersion"),
                ("Filters.member.1.Values.member.1", "6.0"),
            ][..],
            // RBAC's own floor is a version Redis OSS publishes, so only the
            // users listed for it report it.
            vec!["alpha-user", state::DEFAULT_USER_ID, "gamma-user"],
        ),
        (
            "MinimumEngineVersion over both engines' floors",
            &[
                ("Filters.member.1.Name", "MinimumEngineVersion"),
                ("Filters.member.1.Values.member.1", "6.0"),
                ("Filters.member.1.Values.member.2", "7.2"),
            ][..],
            vec![
                "alpha-user",
                "beta-user",
                state::DEFAULT_USER_ID,
                "delta-user",
                "gamma-user",
            ],
        ),
    ] {
        let reply = emulator.call("DescribeUsers", &params(pairs)).await;
        assert_eq!(
            reply.all("UserId"),
            expected,
            "case filter {case}: expected {expected:?}, got {}",
            reply.body
        );
    }

    let unknown = emulator
        .call(
            "DescribeUsers",
            &params(&[
                ("Filters.member.1.Name", "Shoesize"),
                ("Filters.member.1.Values.member.1", "9"),
            ]),
        )
        .await;
    assert_eq!(
        unknown.status, 400,
        "a filter on no property of a user must be refused, got {}",
        unknown.body
    );

    // An id no user has is the modelled not-found, with its own status.
    let missing = emulator
        .call("DescribeUsers", &params(&[("UserId", "no-such-user")]))
        .await;
    assert_eq!(
        missing.status, 404,
        "an unknown id must be answered 404, got {}",
        missing.body
    );
    assert_eq!(
        missing.error_code(),
        "UserNotFound",
        "expected UserNotFound, got {}",
        missing.body
    );
}

#[tokio::test]
async fn describe_pages_over_the_stored_users() {
    let emulator = Emulator::new();
    let ids = ["page-a", "page-b", "page-c", "page-d", "page-e"];
    for id in ids {
        create_user(&emulator, id, "redis").await;
    }
    // Every user the account holds, including the built-in one, in the order a
    // page walk must cover exactly once.
    let mut expected: BTreeSet<String> = ids.iter().map(|id| (*id).to_string()).collect();
    expected.insert(state::DEFAULT_USER_ID.to_string());

    // The axis: the page size, from one record per page to one page holding
    // every record.
    for page_size in [1_usize, 2, 3, expected.len(), expected.len() + 1] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut pairs = params(&[("MaxRecords", &page_size.to_string())]);
            if let Some(token) = marker.as_deref() {
                pairs.extend(params(&[("Marker", token)]));
            }
            let reply = emulator.call("DescribeUsers", &pairs).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: describe must succeed, got {}",
                reply.body
            );
            let page = reply.all("UserId");
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most that many users, got {:?}",
                page
            );
            seen.extend(page.iter().map(|id| (*id).to_string()));
            marker = reply.marker().map(str::to_string);
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case page size {page_size}: the walk must cover every user, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            expected.len(),
            "case page size {page_size}: no user may be reported twice, got {seen:?}"
        );
    }

    // A page size that cannot be one, and a marker this service did not mint,
    // are refused rather than silently served.
    for (case, pairs) in [
        ("zero", &[("MaxRecords", "0")][..]),
        ("negative", &[("MaxRecords", "-1")][..]),
        ("a foreign marker", &[("Marker", "not-a-marker")][..]),
    ] {
        let reply = emulator.call("DescribeUsers", &params(pairs)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {}",
            reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
}
