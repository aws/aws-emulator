// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusals whose WORDING and whose firing RULE a client
//! branches on: the identifier form of a `CacheParameterGroupName` a delete names, the
//! parameter a customer may not change, the tag keys and values a caller may not write,
//! the tag keys a resource does not carry, and a user-group id already taken.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a `CacheParameterGroupName` a DELETE names is held against the service's identifier
//!   grammar BEFORE it is resolved against stored state — a name that breaks the grammar
//!   is the modelled `InvalidParameterValue` (400) stating the member and the constraint,
//!   and only a name the grammar admits reaches the existence and default-namespace
//!   rules;
//! * a parameter the published catalog fixes (one the engine does not let a customer
//!   modify, and one whose value is per cache node type) is refused with the sentence
//!   naming that parameter, through both writers of parameter values;
//! * the reserved tag-key prefix and the published tag character set are each refused
//!   with the RULE's own sentence, identically at every entry point that names tags;
//! * `RemoveTagsFromResource` refuses with the LIST of the keys the resource does not
//!   carry, however many of them the request named, and removes nothing;
//! * a `CreateUserGroup` for an id the account already holds is refused with the full
//!   sentence, remediation clause included — and the group can be deleted, which is how
//!   that collision is reached a second time.
//!
//! Every assertion goes over the wire through [`crate::harness`], so a message that is
//! not computed from the request and the store cannot pass. The expected texts are
//! written out here on purpose: a test that built them from the same helper the handler
//! calls would pass whatever that helper said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::describe_cache_parameters::create_group;
use crate::harness::{params, Emulator, Reply, REGION};
use crate::modify_cache_parameter_group::modify;

/// The sentence ElastiCache refuses a value that is not one of its identifiers with,
/// for the member a cache parameter group delete names.
const IDENTIFIER_RULE: &str = "The parameter CacheParameterGroupName is not a valid \
                               identifier. Identifiers must begin with a letter; must \
                               contain only ASCII letters, digits, and hyphens; and must \
                               not end with a hyphen or contain two consecutive hyphens.";

/// The sentence ElastiCache refuses a tag key under the prefix it reserves for itself
/// with: the rule alone, with no part of the request in it.
const RESERVED_PREFIX_RULE: &str = "Tag keys cannot start with the reserved prefix \"aws:\".";

/// The sentence ElastiCache refuses a tag VALUE outside its published character set
/// with. The symbol list is the service's own spelling of it — the backslash doubled and
/// a closing double quote — and not a tidied transcription.
const TAG_VALUE_CHARSET_RULE: &str = "Tag values may only contain unicode letters, digits, \
                                      whitespace, or one of these symbols: _ . : / = + \\\\ - @\"";

/// [`TAG_VALUE_CHARSET_RULE`] for the other half of the pair.
const TAG_KEY_CHARSET_RULE: &str = "Tag keys may only contain unicode letters, digits, \
                                    whitespace, or one of these symbols: _ . : / = + \\\\ - @\"";

/// The family every group these tests create belongs to, from
/// `CacheParameterGroup$CacheParameterGroupFamily`'s own valid values.
const FAMILY: &str = "redis7";

/// `DeleteCacheParameterGroup` for `name`.
async fn delete_group(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "DeleteCacheParameterGroup",
            &params(&[("CacheParameterGroupName", name)]),
        )
        .await
}

/// `ResetCacheParameterGroup` for `group`, naming `parameters`.
async fn reset(emulator: &Emulator, group: &str, parameters: &[&str]) -> Reply {
    let mut form = params(&[("CacheParameterGroupName", group)]);
    for (index, name) in parameters.iter().enumerate() {
        form.push((
            format!("ParameterNameValues.ParameterNameValue.{}.ParameterName", index + 1),
            (*name).to_string(),
        ));
    }
    emulator.call("ResetCacheParameterGroup", &form).await
}

/// Assert `reply` is the refusal `status` / `code` carrying exactly `message`.
fn assert_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (status, code),
        "case {case:?}: expected {status} {code}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// `AddTagsToResource` on `arn` with one tag.
async fn add_tag(emulator: &Emulator, arn: &str, key: &str, value: &str) -> Reply {
    emulator
        .call(
            "AddTagsToResource",
            &params(&[
                ("ResourceName", arn),
                ("Tags.Tag.1.Key", key),
                ("Tags.Tag.1.Value", value),
            ]),
        )
        .await
}

/// `ListTagsForResource` on `arn`, as `(Key, Value)` pairs in wire order. Every value
/// these tests store is non-empty, so no pair drops out of the scan.
async fn list_tags(emulator: &Emulator, arn: &str) -> Vec<(String, String)> {
    let reply = emulator
        .call("ListTagsForResource", &params(&[("ResourceName", arn)]))
        .await;
    assert_eq!(
        reply.status, 200,
        "ListTagsForResource must answer 200, got {} {}",
        reply.status, reply.body
    );
    let keys = reply.all("Tag/Key");
    let values = reply.all("Tag/Value");
    assert_eq!(
        keys.len(),
        values.len(),
        "every reported tag carries both halves, got {keys:?} and {values:?} in {}",
        reply.body
    );
    keys.into_iter()
        .map(str::to_string)
        .zip(values.into_iter().map(str::to_string))
        .collect()
}

/// `RemoveTagsFromResource` on `arn`, naming `keys`.
async fn remove_tags(emulator: &Emulator, arn: &str, keys: &[&str]) -> Reply {
    let mut form = params(&[("ResourceName", arn)]);
    for (index, key) in keys.iter().enumerate() {
        form.push((format!("TagKeys.member.{}", index + 1), (*key).to_string()));
    }
    emulator.call("RemoveTagsFromResource", &form).await
}

/// Create one parameter group and hand back the ARN its tags hang off.
async fn tagged_group(emulator: &Emulator, name: &str) -> String {
    create_group(emulator, name, FAMILY).await;
    state::parameter_group_arn(REGION, name)
}

/// A name that breaks the identifier grammar is refused before the store is consulted:
/// the rows break one clause of the grammar each, and none of them is a name the account
/// holds or could ever hold.
#[tokio::test]
async fn a_malformed_parameter_group_name_is_refused_as_a_bad_parameter() {
    let emulator = Emulator::new();
    // One group the account really holds, so a row that leaked into the store rules
    // would show up as a changed account below.
    create_group(&emulator, "held-group", FAMILY).await;

    // The axis: WHICH clause of the grammar the name breaks — the leading letter, the
    // character set, the trailing hyphen, the consecutive hyphens, and a name breaking
    // several at once. The `default.<family>` rows are names the account DOES hold, so a
    // store lookup that ran first would answer the default-namespace rule instead.
    let rows: [(&str, &str); 12] = [
        ("a family default group", "default.redis7"),
        ("another family's default group", "default.memcached1.4"),
        ("a family default in another letter case", "DEFAULT.VALKEY8"),
        ("a name starting with a digit", "9x"),
        ("a name starting with a hyphen", "-group"),
        ("two consecutive hyphens", "bad--name"),
        ("a trailing hyphen", "trailing-"),
        ("every clause at once", "9x--bad-"),
        ("an underscore", "under_score"),
        ("a dot in a custom name", "my.group"),
        ("a space", "my group"),
        ("the empty name", ""),
    ];

    for (case, name) in rows {
        let refused = delete_group(&emulator, name).await;
        assert_refusal(case, &refused, 400, "InvalidParameterValue", IDENTIFIER_RULE);
    }

    // Nothing the refusals named was created or removed: the group the account holds is
    // still there and still deletable.
    let deleted = delete_group(&emulator, "held-group").await;
    assert_eq!(
        deleted.status, 200,
        "the refusals must leave the account alone, got {} {}",
        deleted.status, deleted.body
    );
}

/// A name the grammar ADMITS is resolved against the store, which is the other half of
/// the rule: the existence rule answers it, including at the published length bound,
/// while one character past that bound is a bad parameter again.
#[tokio::test]
async fn a_legal_parameter_group_name_is_resolved_against_the_store() {
    let emulator = Emulator::new();
    create_group(&emulator, "a-group-that-exists", FAMILY).await;

    // At the bound and one past it: the 255-character name this service's names are
    // bounded at (`state::PARAMETER_GROUP_NAME_MAX_LEN`).
    let at_bound = "g".repeat(state::PARAMETER_GROUP_NAME_MAX_LEN);
    let past_bound = "g".repeat(state::PARAMETER_GROUP_NAME_MAX_LEN + 1);

    // (case, the name, the status, the code) — every row a LEGAL identifier but the last.
    let rows: [(&str, &str, u16, &str); 6] = [
        (
            "a stored group",
            "a-group-that-exists",
            200,
            "",
        ),
        (
            "a legal name the account does not hold",
            "no-such-group",
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "a legal name with digits and hyphens",
            "group-9-x",
            404,
            "CacheParameterGroupNotFound",
        ),
        (
            "a single-letter name",
            "g",
            404,
            "CacheParameterGroupNotFound",
        ),
        ("a name at the length bound", at_bound.as_str(), 404, "CacheParameterGroupNotFound"),
        (
            "a name one character past the length bound",
            past_bound.as_str(),
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, name, status, code) in rows {
        let reply = delete_group(&emulator, name).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The over-long name is refused for its LENGTH rather than its form, so its message
    // states the bound it is past.
    let too_long = delete_group(&emulator, past_bound.as_str()).await;
    assert!(
        too_long
            .error_message()
            .contains(&state::PARAMETER_GROUP_NAME_MAX_LEN.to_string()),
        "the length refusal must state the bound, got {:?}",
        too_long.error_message()
    );
}

/// Every parameter of `family` a customer may not set a value for — the published rows
/// the engine fixes and the rows whose value is per cache node type — at most `count` of
/// them, taken from the catalog this service publishes rather than written out by hand.
fn unmodifiable(family: &str, count: usize) -> Vec<String> {
    let parameters = state::family_default_parameters(family)
        .unwrap_or_else(|| panic!("case {family}: a published family has a catalog"));
    let mut chosen: Vec<String> = Vec::new();
    for parameter in parameters.values() {
        if parameter.is_modifiable && !parameter.is_node_type_specific() {
            continue;
        }
        if !chosen.contains(&parameter.name) {
            chosen.push(parameter.name.clone());
        }
        if chosen.len() == count {
            break;
        }
    }
    assert_eq!(
        chosen.len(),
        count,
        "case {family}: the family must publish {count} parameters a customer may not \
         change, got {chosen:?}"
    );
    chosen
}

/// A parameter the catalog fixes is refused, by both writers of parameter values, with
/// the sentence naming that parameter.
#[tokio::test]
async fn a_parameter_the_catalog_fixes_cannot_be_modified() {
    let emulator = Emulator::new();

    // The axis: the FAMILY (both engine catalogs, several version lines) and, inside
    // each, several of the parameters it fixes — including the node-type-specific rows,
    // whose value the node's own capacity gives them.
    for family in ["redis7", "redis6.x", "valkey8", "memcached1.6"] {
        let group = format!("fixed-{}", family.replace('.', "-"));
        create_group(&emulator, &group, family).await;

        for name in unmodifiable(family, 3) {
            let expected = format!("The parameter {name} cannot be modified.");
            let case = format!("{family}/{name}");

            // Asking to SET a value for it.
            let set = modify(&emulator, &group, &[(name.as_str(), "1")]).await;
            assert_refusal(&case, &set, 400, "InvalidParameterValue", &expected);

            // And asking to put it back to the engine's own value, which is the same
            // change nothing can make.
            let put_back = reset(&emulator, &group, &[name.as_str()]).await;
            assert_refusal(&case, &put_back, 400, "InvalidParameterValue", &expected);
        }
    }
}

/// A tag key under the reserved prefix is refused with the RULE, at every entry point
/// that names tag keys, whatever the rest of the key is.
#[tokio::test]
async fn a_reserved_tag_key_prefix_is_refused_with_the_rule() {
    let emulator = Emulator::new();
    let arn = tagged_group(&emulator, "reserved-prefix-group").await;

    // The axis: the letter case of the prefix and what follows it — nothing, a plain
    // name, a name carrying the prefix again, and a unicode name.
    let keys = [
        "aws:cost-centre",
        "AWS:Cost-Centre",
        "Aws:aws:nested",
        "aws:",
        "aws:日本語",
    ];

    for key in keys {
        let added = add_tag(&emulator, &arn, key, "v").await;
        assert_refusal(key, &added, 400, "InvalidParameterValue", RESERVED_PREFIX_RULE);

        // The same rule through a create's own `Tags`.
        let created = emulator
            .call(
                "CreateCacheParameterGroup",
                &params(&[
                    ("CacheParameterGroupName", "reserved-prefix-create"),
                    ("CacheParameterGroupFamily", FAMILY),
                    ("Description", "a create carrying a reserved tag key"),
                    ("Tags.Tag.1.Key", key),
                    ("Tags.Tag.1.Value", "v"),
                ]),
            )
            .await;
        assert_refusal(key, &created, 400, "InvalidParameterValue", RESERVED_PREFIX_RULE);

        // And through a REMOVE, which names tag keys too.
        let removed = remove_tags(&emulator, &arn, &[key]).await;
        assert_refusal(key, &removed, 400, "InvalidParameterValue", RESERVED_PREFIX_RULE);
    }

    // A key that merely CONTAINS the prefix is not under it, so it is stored.
    let ok = add_tag(&emulator, &arn, "team-aws:owned", "v").await;
    assert_eq!(
        ok.status, 200,
        "a key that only contains the prefix must be accepted, got {} {}",
        ok.status, ok.body
    );
}

/// A tag key or value carrying a character outside the published set is refused with the
/// published sentence for that half of the pair — and the unicode text the set DOES admit
/// survives the round trip.
#[tokio::test]
async fn a_tag_outside_the_published_character_set_states_the_published_sentence() {
    let emulator = Emulator::new();
    let arn = tagged_group(&emulator, "charset-message-group").await;

    // The axis: WHICH kind of character the set leaves out.
    let outside = [
        ("a pictograph", "party-\u{1F389}-time"),
        ("a percent sign", "100%-cache"),
        ("a comma", "one,two"),
        ("a parenthesis", "cache-(prod)"),
        ("an ampersand", "a&b"),
        ("a double quote", "say-\"what\""),
    ];

    for (case, text) in outside {
        let in_value = add_tag(&emulator, &arn, "charset-key", text).await;
        assert_refusal(
            &format!("{case} in a value"),
            &in_value,
            400,
            "InvalidParameterValue",
            TAG_VALUE_CHARSET_RULE,
        );

        let in_key = add_tag(&emulator, &arn, text, "v").await;
        assert_refusal(
            &format!("{case} in a key"),
            &in_key,
            400,
            "InvalidParameterValue",
            TAG_KEY_CHARSET_RULE,
        );
    }

    // The happy path this check guards: the unicode the set admits is stored and
    // reported back character for character.
    let admitted = [
        ("CJK", "日本語のタグ"),
        ("Cyrillic", "Кириллица"),
        ("accented Latin", "Ünicode café"),
        ("letters, digits and the published symbols", "a_b.c:d/e=f+g-h@i 42"),
    ];
    for (index, (case, text)) in admitted.iter().enumerate() {
        let key = format!("unicode-{index}");
        let added = add_tag(&emulator, &arn, &key, text).await;
        assert_eq!(
            added.status, 200,
            "case {case:?}: AddTagsToResource must answer 200, got {} {}",
            added.status, added.body
        );
        let held = list_tags(&emulator, &arn).await;
        assert!(
            held.contains(&(key.clone(), (*text).to_string())),
            "case {case:?}: expected {text:?} stored under {key}, got {held:?}"
        );
    }
}

/// A remove naming keys the resource does not carry is refused with the LIST of those
/// keys, however many there are, and removes nothing.
#[tokio::test]
async fn removing_absent_tag_keys_reports_every_absent_key() {
    let emulator = Emulator::new();
    let arn = tagged_group(&emulator, "absent-keys-group").await;
    for (key, value) in [("held-one", "1"), ("held-two", "2")] {
        let added = add_tag(&emulator, &arn, key, value).await;
        assert_eq!(
            added.status, 200,
            "the setup tag {key} must be stored, got {} {}",
            added.status, added.body
        );
    }
    let before = list_tags(&emulator, &arn).await;

    // The axis: HOW MANY of the named keys are absent, and where they sit in the list.
    let rows: [(&str, &[&str], &str); 5] = [
        ("one absent key", &["never-existed"], "never-existed"),
        (
            "two absent keys",
            &["never-existed", "nor-this"],
            "never-existed,nor-this",
        ),
        (
            "an absent key after a held one",
            &["held-one", "never-existed"],
            "never-existed",
        ),
        (
            "an absent key before a held one",
            &["never-existed", "held-two"],
            "never-existed",
        ),
        (
            "held and absent keys interleaved",
            &["held-one", "never-existed", "held-two", "nor-this"],
            "never-existed,nor-this",
        ),
    ];

    for (case, keys, absent) in rows {
        let refused = remove_tags(&emulator, &arn, keys).await;
        assert_refusal(
            case,
            &refused,
            404,
            "TagNotFound",
            &format!("Failing request. These tags are not present : {absent}"),
        );
        let after = list_tags(&emulator, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must remove nothing, expected {before:?}, got {after:?}"
        );
    }

    // The keys the resource DOES carry are removed when every named key is present.
    let removed = remove_tags(&emulator, &arn, &["held-one", "held-two"]).await;
    assert_eq!(
        removed.status, 200,
        "removing the keys the resource carries must answer 200, got {} {}",
        removed.status, removed.body
    );
    let after = list_tags(&emulator, &arn).await;
    assert!(
        after.is_empty(),
        "expected both tags removed, got {after:?}"
    );
}

/// `CreateUserGroup` for an id the account already holds is refused with the whole
/// sentence, remediation clause included — whatever the id and whatever members the
/// second create names.
#[tokio::test]
async fn recreating_a_user_group_is_refused_with_the_remediation_clause() {
    let emulator = Emulator::new();

    // One create: the id, the members it names, and the engine. `default` is the user
    // the service itself owns ("a user group must contain the default user"), so no row
    // depends on a user a test created.
    let rows: [(&str, &[&str], &str); 4] = [
        ("group-one", &["default"], "redis"),
        ("GroupTwo", &["default"], "redis"),
        ("group3", &["default"], "valkey"),
        ("a-rather-longer-group-id-0123456789", &["default"], "redis"),
    ];

    for (id, members, user_engine) in rows {
        let mut form = params(&[("UserGroupId", id), ("Engine", user_engine)]);
        for (index, member) in members.iter().enumerate() {
            form.push((
                format!("UserIds.member.{}", index + 1),
                (*member).to_string(),
            ));
        }
        let created = emulator.call("CreateUserGroup", &form).await;
        assert_eq!(
            created.status, 200,
            "case {id:?}: the first create must answer 200, got {} {}",
            created.status, created.body
        );

        // The same id again, and the same id with another membership: both collide with
        // the group that is already there.
        for (case, again) in [
            ("the same request", form.clone()),
            (
                "the same id with another membership",
                params(&[
                    ("UserGroupId", id),
                    ("Engine", user_engine),
                    ("UserIds.member.1", "default"),
                ]),
            ),
        ] {
            let refused = emulator.call("CreateUserGroup", &again).await;
            assert_refusal(
                &format!("{id}: {case}"),
                &refused,
                400,
                "UserGroupAlreadyExists",
                &format!("User group {id} already exists. Please retry with another name."),
            );
        }
    }
}

/// `DeleteUserGroup` — the stub the collision above is observed through (its own report
/// is `20261024T075915Z-elasticache-rbac-surface-deleteusergroup-and-modifyuser-answer-501.md`):
/// the group leaves the account, read back through `DescribeUserGroups`, and the id is
/// free for a create again once the removal the clock owes has fallen due.
#[tokio::test]
async fn a_user_group_is_deleted_and_read_back_as_gone() {
    let emulator = Emulator::new();

    /// Create one group and hand back its reply.
    async fn create(emulator: &Emulator, id: &str, user_engine: &str) -> Reply {
        emulator
            .call(
                "CreateUserGroup",
                &params(&[
                    ("UserGroupId", id),
                    ("Engine", user_engine),
                    ("UserIds.member.1", "default"),
                ]),
            )
            .await
    }
    async fn delete(emulator: &Emulator, id: &str) -> Reply {
        emulator
            .call("DeleteUserGroup", &params(&[("UserGroupId", id)]))
            .await
    }
    async fn describe(emulator: &Emulator, id: &str) -> Reply {
        emulator
            .call("DescribeUserGroups", &params(&[("UserGroupId", id)]))
            .await
    }

    // A group the account does not hold: the modelled 404, before any group exists.
    for id in ["never-created", "Never-Created-Either"] {
        let refused = delete(&emulator, id).await;
        assert_refusal(
            id,
            &refused,
            404,
            "UserGroupNotFound",
            &format!("User group {id} not found."),
        );
    }

    // The axis: the engine access control is listed for, and the spelling of the id.
    for (id, user_engine) in [("doomed-group", "redis"), ("DoomedTwo", "valkey")] {
        let created = create(&emulator, id, user_engine).await;
        assert_eq!(
            created.status, 200,
            "case {id:?}: the create must answer 200, got {} {}",
            created.status, created.body
        );
        // A group the clock has not finished creating is not in an active state, so it
        // takes no change yet.
        let too_early = delete(&emulator, id).await;
        assert_eq!(
            (too_early.status, too_early.error_code()),
            (400, "InvalidUserGroupState"),
            "case {id:?}: a delete inside the create span must be the modelled state fault, \
             got {} {}",
            too_early.status,
            too_early.body
        );
        // Reading the group back is answered once the create has settled it active,
        // which is the state a delete is accepted from.
        let active = describe(&emulator, id).await;
        assert_eq!(
            active.text("Status"),
            Some("active"),
            "case {id:?}: the read after the create must report the group active, got {}",
            active.body
        );

        let deleted = delete(&emulator, id).await;
        assert_eq!(
            deleted.status, 200,
            "case {id:?}: the delete must answer 200, got {} {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            (deleted.text("UserGroupId"), deleted.text("Status")),
            (Some(id), Some("deleting")),
            "case {id:?}: the delete must answer with the stored group on its way out, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.all("UserIds/member"),
            vec!["default"],
            "case {id:?}: the answer must carry the stored membership, got {}",
            deleted.body
        );
        assert!(
            deleted
                .text("ARN")
                .is_some_and(|arn| arn.ends_with(&format!("usergroup:{id}"))),
            "case {id:?}: the answer must carry the stored ARN, got {}",
            deleted.body
        );

        // The read that follows the delete is answered once the removal the clock owed has
        // been carried through: every reader answers the modelled not-found and the id is
        // free again.
        let after = describe(&emulator, id).await;
        assert_eq!(
            (after.status, after.error_code()),
            (404, "UserGroupNotFound"),
            "case {id:?}: the deleted group must be gone, got {} {}",
            after.status,
            after.body
        );
        let again = delete(&emulator, id).await;
        assert_refusal(
            id,
            &again,
            404,
            "UserGroupNotFound",
            &format!("User group {id} not found."),
        );
        let recreated = create(&emulator, id, user_engine).await;
        assert_eq!(
            recreated.status, 200,
            "case {id:?}: the id must be free to create again, got {} {}",
            recreated.status, recreated.body
        );
    }
}
