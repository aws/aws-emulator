// SPDX-License-Identifier: CC0-1.0

//! The policy-SIZE quota of an entity that embeds inline policies, end to end over the
//! awsQuery wire.
//!
//! RULE under test: IAM measures a policy document against the embedding entity's
//! published policy-size quota over its NON-WHITESPACE characters only — AWS's "IAM and
//! STS quotas" page states that IAM "doesn't count whitespace when calculating the size
//! of a policy against this limitation" — so a document that fits the quota once its
//! whitespace is left out is embedded however it is formatted, however long its raw text
//! is; a put that would carry the entity's inline policies past that quota is refused
//! HTTP 409 `LimitExceeded` worded `Maximum policy size of {quota} bytes exceeded for
//! {entity kind} {name}` — the quota the entity kind publishes, the kind's own word, and
//! the name taken from the request — and embeds nothing, leaving a policy already stored
//! under that name exactly as it was.
//!
//! The rule is the same one for every entity kind that embeds inline policies, so the
//! tables below span all three (`PutUserPolicy`, `PutRolePolicy`, `PutGroupPolicy`, each
//! read back through its own `Get...Policy`) and, within each, the way a document is
//! formatted: no whitespace at all, one space per token boundary, a four-kilobyte run of
//! spaces, and the tab, line feed and carriage return `policyDocumentType`'s pattern
//! admits (`smithy_query(query="operation", operation="PutGroupPolicy")`). Each size row
//! sits on a published bound: one significant character under it, exactly on it, and one
//! past it.
//!
//! One bound here is the emulator's own rather than IAM's: because the quota ignores a
//! document's whitespace, what an entity's inline policies can make this store HOLD is
//! not bounded by the quota, so the aggregate raw text one entity retains is capped at
//! `state::MAX_INLINE_POLICY_TEXT_CHARACTERS` and a put past it is refused with the same
//! modelled `LimitExceededException` in that ceiling's own words. The last suite below
//! asserts it.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a policy that was PERSISTED rather than a handler called directly.
//! The awsQuery harness (`router`, `call`, `element`, `error_code`) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy with a single statement, which a fixture role needs to be created at
/// all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The significant (non-whitespace) size of the shortest document [`document_of`] builds,
/// which is what its padding is measured on top of.
const SIGNIFICANT_FLOOR: usize = 26;

/// One entity kind that embeds inline policies: how it is created, the calls that embed
/// and report a policy, the quota its inline policies share, and the word IAM's refusal
/// names the kind by.
struct Kind {
    /// The word the refusal uses for this entity kind.
    kind: &'static str,
    /// The aggregate policy-size quota this kind publishes.
    quota: usize,
    /// The published figure the quota must equal, as AWS's "IAM and STS quotas" page
    /// words it, so a row is pinned to the page rather than to the constant.
    published: &'static str,
    /// The create call and the member it names the entity by.
    create_action: &'static str,
    name_member: &'static str,
    /// The put and the read for an inline policy of this kind.
    put_action: &'static str,
    get_action: &'static str,
}

/// Every entity kind that embeds inline policies, with the quota AWS's "IAM and STS
/// quotas" page publishes for it: "User policy size cannot exceed 2,048 characters",
/// "Role policy size cannot exceed 10,240 characters", "Group policy size cannot exceed
/// 5,120 characters".
const KINDS: &[Kind] = &[
    Kind {
        kind: "user",
        quota: state::MAX_USER_INLINE_POLICY_CHARACTERS,
        published: "User policy size cannot exceed 2,048 characters",
        create_action: "CreateUser",
        name_member: "UserName",
        put_action: "PutUserPolicy",
        get_action: "GetUserPolicy",
    },
    Kind {
        kind: "role",
        quota: state::MAX_ROLE_INLINE_POLICY_CHARACTERS,
        published: "Role policy size cannot exceed 10,240 characters",
        create_action: "CreateRole",
        name_member: "RoleName",
        put_action: "PutRolePolicy",
        get_action: "GetRolePolicy",
    },
    Kind {
        kind: "group",
        quota: state::MAX_GROUP_INLINE_POLICY_CHARACTERS,
        published: "Group policy size cannot exceed 5,120 characters",
        create_action: "CreateGroup",
        name_member: "GroupName",
        put_action: "PutGroupPolicy",
        get_action: "GetGroupPolicy",
    },
];

/// The figure each kind's quota must be, read off the wording on AWS's quotas page that
/// [`Kind::published`] carries.
fn published_figure(published: &str) -> usize {
    let digits: String = published
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect();
    digits
        .replace(',', "")
        .parse()
        .unwrap_or_else(|_| panic!("the published wording must carry a figure: {published:?}"))
}

/// How a document is formatted: the whitespace run placed at each of its token
/// boundaries, as `unit` repeated `repeat` times. IAM counts none of it.
struct Formatting {
    case: &'static str,
    unit: &'static str,
    repeat: usize,
}

impl Formatting {
    /// The run this formatting places at one token boundary.
    fn whitespace(&self) -> String {
        self.unit.repeat(self.repeat)
    }
}

/// The formattings every size row is asserted under: a compact document, one spaced at
/// every token boundary, one carrying a four-hundred-character run of spaces per
/// boundary (so its raw text runs thousands of characters past the quota), and one
/// indented with the tab, line feed and carriage return `policyDocumentType` admits.
const FORMATTINGS: &[Formatting] = &[
    Formatting {
        case: "compact, no whitespace at all",
        unit: "",
        repeat: 0,
    },
    Formatting {
        case: "one space per token boundary",
        unit: " ",
        repeat: 1,
    },
    Formatting {
        case: "a long run of spaces per token boundary",
        unit: " ",
        repeat: 400,
    },
    Formatting {
        case: "indented with tab, line feed and carriage return",
        unit: "\t\r\n",
        repeat: 2,
    },
];

/// A policy document whose SIGNIFICANT (non-whitespace) size is exactly `significant`
/// characters, with `whitespace` placed at each of its ten token boundaries — so its RAW
/// size grows with the formatting while the size IAM measures does not.
///
/// The document is a JSON object carrying `Statement`, which is what the IAM policy
/// grammar asks of it, with the significant padding inside a `Sid` string.
fn document_of(significant: usize, whitespace: &str) -> String {
    assert!(
        significant >= SIGNIFICANT_FLOOR,
        "a padded document cannot be shorter than {SIGNIFICANT_FLOOR} significant characters"
    );
    let padding = "a".repeat(significant - SIGNIFICANT_FLOOR);
    let ws = whitespace;
    let document =
        format!("{{{ws}\"Statement\":{ws}[{ws}{{{ws}\"Sid\":{ws}\"{padding}\"{ws}}}{ws}]{ws}}}{ws}");
    // Measured here the way the rule states it, independently of the crate's own
    // measure, so the fixture is what it claims to be.
    let measured = document.chars().filter(|c| !c.is_whitespace()).count();
    assert_eq!(
        measured, significant,
        "the fixture must carry exactly {significant} significant characters, got {measured}"
    );
    document
}

/// Create the entity `name` of this kind, failing the test when the create a row depends
/// on does not succeed.
async fn create_entity(router: &MultiServiceRouter, kind: &Kind, name: &str) {
    let mut params = vec![(kind.name_member, name)];
    if kind.create_action == "CreateRole" {
        params.push(("AssumeRolePolicyDocument", TRUST_POLICY));
    }
    let (status, body) = call(router, kind.create_action, &params).await;
    assert_eq!(
        status, 200,
        "case {name:?}: {} must succeed, got {status}: {body}",
        kind.create_action
    );
}

/// Embed `document` in `name` under `policy_name`.
async fn put_policy(
    router: &MultiServiceRouter,
    kind: &Kind,
    name: &str,
    policy_name: &str,
    document: &str,
) -> (u16, String) {
    call(
        router,
        kind.put_action,
        &[
            (kind.name_member, name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await
}

/// Read one embedded policy back.
async fn get_policy(
    router: &MultiServiceRouter,
    kind: &Kind,
    name: &str,
    policy_name: &str,
) -> (u16, String) {
    call(
        router,
        kind.get_action,
        &[(kind.name_member, name), ("PolicyName", policy_name)],
    )
    .await
}

/// The refusal IAM words for a put past the quota of `kind` for the entity `name`.
fn expected_refusal(kind: &Kind, name: &str) -> String {
    format!(
        "Maximum policy size of {} bytes exceeded for {} {name}",
        kind.quota, kind.kind
    )
}

/// A document whose significant size is `significant` characters and whose RAW length is
/// exactly `raw`, the difference being one run of spaces IAM does not count.
fn document_of_raw(significant: usize, raw: usize) -> String {
    let compact = document_of(significant, "");
    let padding = raw
        .checked_sub(compact.chars().count())
        .expect("a raw length at least as long as the compact document");
    let document = format!("{compact}{}", " ".repeat(padding));
    assert_eq!(
        document.chars().count(),
        raw,
        "the fixture must be exactly {raw} raw characters"
    );
    document
}

/// The refusal the store's own ceiling on retained policy text carries for `kind`'s
/// entity `name`.
fn expected_retention_refusal(kind: &Kind, name: &str) -> String {
    format!(
        "The inline policies of {} {name} cannot hold more than {} characters of policy text \
         in this emulator",
        kind.kind,
        state::MAX_INLINE_POLICY_TEXT_CHARACTERS
    )
}

/// RULE: the whitespace the quota ignores is still TEXT the store has to hold, so what one
/// entity's inline policies may retain is bounded on its own: a document whose raw length
/// sits on that ceiling is embedded and reads back verbatim, and the next put — of any
/// size, however far inside the policy-size quota it is — is refused 409 `LimitExceeded`
/// and leaves what is stored alone.
#[tokio::test]
async fn the_text_one_entity_retains_is_bounded() {
    let ceiling = state::MAX_INLINE_POLICY_TEXT_CHARACTERS;

    let router = router();
    for kind in KINDS {
        let name = format!("{}-retained", kind.kind);
        create_entity(&router, kind, &name).await;

        // On the ceiling: a document far inside the policy-size quota (its significant
        // size is the smallest the grammar admits) whose formatting takes it to exactly
        // what one entity may retain.
        let at_ceiling = document_of_raw(SIGNIFICANT_FLOOR, ceiling);
        let (status, body) = put_policy(&router, kind, &name, "AtCeiling", &at_ceiling).await;
        assert_eq!(
            status, 200,
            "case {:?}: a document on the retention ceiling must be embedded, got {status}: \
             {body}",
            kind.kind
        );
        let (status, read_body) = get_policy(&router, kind, &name, "AtCeiling").await;
        assert_eq!(
            status, 200,
            "case {:?}: the embedded document must read back, got {status}: {read_body}",
            kind.kind
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
            at_ceiling,
            "case {:?}: the document must read back verbatim, whitespace and all",
            kind.kind
        );

        // Rows: the next put, of a document whose significant size is nowhere near the
        // policy-size quota — the shortest the grammar admits, and one carrying its own
        // formatting — is past the retention ceiling either way.
        for (case, document) in [
            ("the shortest document the grammar admits", document_of(SIGNIFICANT_FLOOR, "")),
            ("another formatted document", document_of_raw(SIGNIFICANT_FLOOR, 4_096)),
        ] {
            let (status, body) = put_policy(&router, kind, &name, "OneTooMuch", &document).await;
            assert_eq!(
                status, 409,
                "case {:?} {case:?}: a put past the retention ceiling must be refused, got \
                 {status}: {body}",
                kind.kind
            );
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {:?} {case:?}: expected the modelled awsQuery code, got {body}",
                kind.kind
            );
            assert_eq!(
                element(&body, "Message"),
                expected_retention_refusal(kind, &name),
                "case {:?} {case:?}: expected the retention refusal, got {body}",
                kind.kind
            );
            let (status, read_body) = get_policy(&router, kind, &name, "OneTooMuch").await;
            assert_eq!(
                status, 404,
                "case {:?} {case:?}: a refused embed must persist nothing, got {status}: \
                 {read_body}",
                kind.kind
            );
        }

        // What was already embedded is untouched by the refusals.
        let (status, read_body) = get_policy(&router, kind, &name, "AtCeiling").await;
        assert_eq!(
            status, 200,
            "case {:?}: the embedded document must survive, got {status}: {read_body}",
            kind.kind
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
            at_ceiling,
            "case {:?}: a refused put must not have touched it",
            kind.kind
        );

        // Replacing the document on the ceiling with another on the ceiling is inside it,
        // because the text it replaces is no longer retained.
        let replacement = document_of_raw(SIGNIFICANT_FLOOR + 1, ceiling);
        let (status, body) = put_policy(&router, kind, &name, "AtCeiling", &replacement).await;
        assert_eq!(
            status, 200,
            "case {:?}: a replacement must be measured without the text it replaces, got \
             {status}: {body}",
            kind.kind
        );
    }
}

/// RULE: the quota is measured over the document's non-whitespace characters, so a
/// document at or under it is embedded however much whitespace it carries, and one a
/// single significant character past it is refused 409 `LimitExceeded` with IAM's
/// sentence and embeds nothing.
#[tokio::test]
async fn the_quota_is_measured_without_whitespace() {
    let router = router();
    for kind in KINDS {
        assert_eq!(
            kind.quota,
            published_figure(kind.published),
            "case {:?}: the quota must be the published figure of {:?}, got {}",
            kind.kind,
            kind.published,
            kind.quota
        );

        for (formatting_index, formatting) in FORMATTINGS.iter().enumerate() {
            // Rows on the published bound: one significant character under it, exactly
            // on it, and one past it.
            for (row, significant, expected_status) in [
                ("one significant character under the bound", kind.quota - 1, 200),
                ("exactly the bound", kind.quota, 200),
                ("one significant character past the bound", kind.quota + 1, 409),
            ] {
                let case = format!("{} {row} {}", kind.kind, formatting.case);
                let name = format!("{}-{formatting_index}-{significant}", kind.kind);
                create_entity(&router, kind, &name).await;
                let document = document_of(significant, &formatting.whitespace());
                let (status, body) = put_policy(&router, kind, &name, "Sized", &document).await;
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: a document of {significant} significant characters \
                     (raw length {}) must answer {expected_status}, got {status}: {body}",
                    document.chars().count()
                );

                let (read_status, read_body) = get_policy(&router, kind, &name, "Sized").await;
                if expected_status == 200 {
                    assert_eq!(
                        read_status, 200,
                        "case {case:?}: the accepted policy must read back, got {read_status}: \
                         {read_body}"
                    );
                    assert_eq!(
                        crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
                        document,
                        "case {case:?}: the document must read back exactly as it was \
                         embedded, got {read_body}"
                    );
                } else {
                    assert_eq!(
                        error_code(&body),
                        "LimitExceeded",
                        "case {case:?}: expected the modelled awsQuery code, got {body}"
                    );
                    assert_eq!(
                        element(&body, "Message"),
                        expected_refusal(kind, &name),
                        "case {case:?}: expected IAM's wording, got {body}"
                    );
                    assert_eq!(
                        read_status, 404,
                        "case {case:?}: a refused embed must persist nothing, got \
                         {read_status}: {read_body}"
                    );
                }
            }
        }
    }
}

/// RULE: a refused put leaves a policy already stored under that name exactly as it was —
/// the replacement is measured, not applied.
#[tokio::test]
async fn a_refused_replacement_leaves_the_stored_policy_alone() {
    let router = router();
    for kind in KINDS {
        // Rows: the formatting of the document that is already stored, so the survivor is
        // read back whatever its text looks like.
        for (formatting_index, formatting) in FORMATTINGS.iter().enumerate() {
            let case = format!("{} {}", kind.kind, formatting.case);
            let name = format!("{}-replace-{formatting_index}", kind.kind);
            create_entity(&router, kind, &name).await;

            let stored = document_of(kind.quota, &formatting.whitespace());
            let (status, body) = put_policy(&router, kind, &name, "AtLimit", &stored).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a document at the bound must be embedded, got {status}: {body}"
            );

            let over = document_of(kind.quota + 1, &formatting.whitespace());
            let (status, body) = put_policy(&router, kind, &name, "AtLimit", &over).await;
            assert_eq!(
                status, 409,
                "case {case:?}: a replacement past the bound must be refused, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                expected_refusal(kind, &name),
                "case {case:?}: expected IAM's wording, got {body}"
            );

            let (status, read_body) = get_policy(&router, kind, &name, "AtLimit").await;
            assert_eq!(
                status, 200,
                "case {case:?}: the stored policy must survive, got {status}: {read_body}"
            );
            assert_eq!(
                crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
                stored,
                "case {case:?}: the refused replacement must not have been applied, got \
                 {read_body}"
            );
        }
    }
}

/// RULE: the quota is on the AGGREGATE of the entity's inline policies, measured the same
/// way — several heavily formatted documents whose significant sizes sum to the quota are
/// all embedded, the next policy of any size is refused, and replacing one of them with a
/// document of its own size is still inside the bound.
#[tokio::test]
async fn the_aggregate_is_measured_without_whitespace() {
    // A run of spaces each aggregate row's documents carry, so every stored document's
    // raw size is far past the quota while the measured sum sits exactly on it.
    let whitespace = FORMATTINGS[2].whitespace();

    let router = router();
    for kind in KINDS {
        let name = format!("{}-aggregate", kind.kind);
        create_entity(&router, kind, &name).await;

        // Rows: how the quota is split across the policies the entity embeds — two halves
        // and three thirds of what is left, the last row taking the remainder so the sum
        // is exactly the quota.
        let splits = [kind.quota / 2, kind.quota / 4, kind.quota - kind.quota / 2 - kind.quota / 4];
        for (index, significant) in splits.iter().enumerate() {
            let case = format!("{} part {index} of {significant} significant characters", kind.kind);
            let policy_name = format!("part-{index}");
            let document = document_of(*significant, &whitespace);
            let (status, body) = put_policy(&router, kind, &name, &policy_name, &document).await;
            assert_eq!(
                status, 200,
                "case {case:?}: policies summing to the bound must be embedded, got {status}: \
                 {body}"
            );
        }

        // The next policy — even the shortest document the grammar admits — is past the
        // aggregate.
        let (status, body) = put_policy(
            &router,
            kind,
            &name,
            "one-too-many",
            &document_of(SIGNIFICANT_FLOOR, &whitespace),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: a policy past the aggregate bound must be refused, got {status}: {body}",
            kind.kind
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {:?}: expected the modelled awsQuery code, got {body}",
            kind.kind
        );
        assert_eq!(
            element(&body, "Message"),
            expected_refusal(kind, &name),
            "case {:?}: expected IAM's wording, got {body}",
            kind.kind
        );

        // Replacing one of the stored policies with a document of its own significant
        // size is still inside the bound, because the document it replaces does not count
        // twice — and the formatting of neither counts at all.
        let replacement = document_of(kind.quota / 2, &FORMATTINGS[3].whitespace());
        let (status, body) = put_policy(&router, kind, &name, "part-0", &replacement).await;
        assert_eq!(
            status, 200,
            "case {:?}: a replacement must be measured without the document it replaces, got \
             {status}: {body}",
            kind.kind
        );
        let (status, read_body) = get_policy(&router, kind, &name, "part-0").await;
        assert_eq!(
            status, 200,
            "case {:?}: the replacement must read back, got {status}: {read_body}",
            kind.kind
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&read_body, "PolicyDocument"),
            replacement,
            "case {:?}: the latest document must be the stored one, got {read_body}",
            kind.kind
        );
    }
}
