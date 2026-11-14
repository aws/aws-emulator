// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH rule an ElastiCache refusal states, and in whose
//! words.
//!
//! A refusal's message is where the service states the rule that was broken, so
//! a client, an SDK waiter or a message-matching fixture branches on it. The
//! rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a `CacheParameterGroupFamily` outside the published vocabulary is refused
//!   with the family rule, naming the requested family and nothing else, at
//!   every operation that resolves a family;
//! * a `CacheParameterGroupName` the service's identifier grammar does not admit
//!   is refused as the malformed parameter it is BEFORE the name is resolved
//!   against stored groups, so a reset of a name that resolves to a default
//!   group states the grammar, not the default-group rule, while a legal name
//!   the account does not hold still reaches the modelled 404;
//! * an `Engine` without the access-control (RBAC) feature is refused with the
//!   CAPABILITY rule — the engine version access control exists from — at every
//!   operation that creates or changes a user or a user group, with no part of
//!   the request in it;
//! * an `Engine` a serverless cache cannot run is refused with the serverless
//!   capability rule, at both the create and the modify;
//! * a tagging call's `ResourceName` that is not ARN-shaped is refused with the
//!   STRUCTURAL rule the service applied — the component count — and echoes no
//!   caller value, identically at all three tagging operations;
//! * `CreateSnapshot` words a cache cluster it cannot resolve as the service
//!   words it there (prose), while the surfaces that merely resolve a
//!   `CacheClusterId` keep the shape's own spelling;
//! * a user group that cannot be DELETED is refused with the statuses a delete
//!   admits, the group's current status read out of the store.
//!
//! Every assertion goes over the wire through [`crate::harness`], so a message
//! that is not computed from the request and the store cannot pass. The expected
//! texts are written out here on purpose: a test that built them from the same
//! helper the handler calls would pass whatever that helper said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::create_available_cluster;
use crate::harness::{params, Emulator, Reply, REGION};
use crate::resource_tags::{add_tags, list_tags, remove_tags};

/// The account id every generated emulator presents, which every ARN it mints
/// carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The sentence ElastiCache refuses a `CacheParameterGroupName` that is not one
/// of its identifiers with: wholly fixed text naming the member and the grammar.
const IDENTIFIER_RULE: &str = "The parameter CacheParameterGroupName is not a valid identifier. \
                               Identifiers must begin with a letter; must contain only ASCII \
                               letters, digits, and hyphens; and must not end with a hyphen or \
                               contain two consecutive hyphens.";

/// The sentence ElastiCache refuses an engine that has no access control with:
/// the capability rule, with no part of the request in it.
const ACCESS_CONTROL_RULE: &str = "Access control options are available for Redis version 6.0 \
                                   onwards.";

/// The sentence ElastiCache refuses an engine that cannot run a serverless cache
/// with.
const SERVERLESS_RULE: &str = "Specified engine does not support serverless cache.";

/// The structural rule a tagging call applies to a `ResourceName` before it
/// reads any field of it: an ElastiCache ARN has seven colon-separated
/// components.
const ARN_COMPONENTS_RULE: &str = "Input ARN string does not have 7 components.";

/// What a `ResourceName` carrying the components but naming no ElastiCache
/// resource is refused with: the rule the modelled `InvalidARNFault` states
/// (`smithy_query(query="shape", shape="InvalidARNFault")`).
const ARN_RESOURCE_RULE: &str =
    "The requested Amazon Resource Name (ARN) does not refer to an existing resource.";

/// Assert `reply` is the refusal `status` / `code` carrying exactly `message`.
fn refused(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected {code}, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// `DescribeEngineDefaultParameters` for `family`.
async fn engine_defaults(emulator: &Emulator, family: &str) -> Reply {
    emulator
        .call(
            "DescribeEngineDefaultParameters",
            &params(&[("CacheParameterGroupFamily", family)]),
        )
        .await
}

/// `CreateCacheParameterGroup` naming `family`.
async fn create_parameter_group(emulator: &Emulator, name: &str, family: &str) -> Reply {
    emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", name),
                ("CacheParameterGroupFamily", family),
                ("Description", "a group for the family rule"),
            ]),
        )
        .await
}

#[tokio::test]
async fn a_family_outside_the_published_vocabulary_is_refused_with_the_family_rule() {
    let emulator = Emulator::new();

    // The axis: HOW the value misses the published vocabulary — an engine the
    // service does not run, a published engine with an unpublished version line,
    // a line written in a form no family uses, the letter case of a published
    // family (the family names are lowercase), and the empty value.
    let unpublished = [
        "not-a-real-family",
        "redis99",
        "redis7.0",
        "REDIS7",
        "postgres14",
        "",
    ];
    for (index, family) in unpublished.into_iter().enumerate() {
        let expected = format!("CacheParameterGroupFamily {family} is not a valid parameter group family.");
        let described = engine_defaults(&emulator, family).await;
        refused(
            &format!("DescribeEngineDefaultParameters({family})"),
            &described,
            400,
            "InvalidParameterValue",
            &expected,
        );
        // The same vocabulary decides the family a group may be created for, so
        // the create states the same rule.
        let created =
            create_parameter_group(&emulator, &format!("family-rule-{index}"), family).await;
        refused(
            &format!("CreateCacheParameterGroup({family})"),
            &created,
            400,
            "InvalidParameterValue",
            &expected,
        );
    }

    // Every family the service publishes is served by both — including the
    // entries the report that motivated this test never cited — so the refusal
    // above is the vocabulary speaking and not a blanket rejection.
    for (index, family) in state::PARAMETER_GROUP_FAMILIES.into_iter().enumerate() {
        let described = engine_defaults(&emulator, family).await;
        assert_eq!(
            described.status, 200,
            "case {family}: DescribeEngineDefaultParameters must answer 200, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("CacheParameterGroupFamily"),
            Some(family),
            "case {family}: the defaults must be reported for the family asked for, got {}",
            described.body
        );
        let created =
            create_parameter_group(&emulator, &format!("published-{index}"), family).await;
        assert_eq!(
            created.status, 200,
            "case {family}: CreateCacheParameterGroup must answer 200, got {} {}",
            created.status, created.body
        );
    }
}

/// `ResetCacheParameterGroup` asking for the whole of `group` to be reset.
async fn reset_all(emulator: &Emulator, group: &str) -> Reply {
    emulator
        .call(
            "ResetCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", group),
                ("ResetAllParameters", "true"),
            ]),
        )
        .await
}

#[tokio::test]
async fn a_parameter_group_name_is_held_against_the_grammar_before_it_is_resolved() {
    let emulator = Emulator::new();

    // The axis: WHICH clause of the grammar the name breaks — a character
    // outside the set (the '.' every service-owned default group's name
    // carries, so a name that RESOLVES to a stored group is in this table), a
    // leading non-letter, a trailing hyphen, two consecutive hyphens, an
    // underscore, and the empty name.
    let malformed = [
        format!("default.{}", state::PARAMETER_GROUP_FAMILIES[0]),
        format!("default.{}", state::PARAMETER_GROUP_FAMILIES[9]),
        "default.".to_string(),
        "dots.not.allowed".to_string(),
        "9starts-with-a-digit".to_string(),
        "ends-with-a-hyphen-".to_string(),
        "two--hyphens".to_string(),
        "under_scores".to_string(),
        String::new(),
    ];
    for name in &malformed {
        let reply = reset_all(&emulator, name).await;
        refused(name, &reply, 400, "InvalidParameterValue", IDENTIFIER_RULE);
    }

    // The grammar decides the FORM, not the existence: a name the grammar admits
    // and the account does not hold still reaches the modelled 404, and the
    // group that does exist is reset.
    let absent = reset_all(&emulator, "never-created").await;
    refused(
        "a legal name the account does not hold",
        &absent,
        404,
        "CacheParameterGroupNotFound",
        "CacheParameterGroupnot found: never-created",
    );
    let created = create_parameter_group(&emulator, "resettable", "redis7").await;
    assert_eq!(
        created.status, 200,
        "CreateCacheParameterGroup must answer 200, got {} {}",
        created.status, created.body
    );
    let reset = reset_all(&emulator, "resettable").await;
    assert_eq!(
        reset.status, 200,
        "a legal name the account holds must be reset, got {} {}",
        reset.status, reset.body
    );

    // A refused reset left every default group in place, which is what a
    // following read of one reports.
    let still_there = engine_defaults(&emulator, "redis7").await;
    assert_eq!(
        still_there.status, 200,
        "the refusals must have changed nothing, got {} {}",
        still_there.status, still_there.body
    );
}

/// The four operations that create or change a user or a user group, as
/// `(case, Action, the form pairs beside `Engine`)`.
fn access_control_writers(user: &str, group: &str) -> [(&'static str, Vec<(String, String)>); 4] {
    [
        (
            "CreateUser",
            params(&[
                ("UserId", "fresh-user"),
                ("UserName", "fresh-user"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        ),
        (
            "CreateUserGroup",
            {
                let mut pairs = params(&[("UserGroupId", "fresh-group")]);
                pairs.push(("UserIds.member.1".to_string(), "default".to_string()));
                pairs
            },
        ),
        ("ModifyUser", params(&[("UserId", user)])),
        ("ModifyUserGroup", params(&[("UserGroupId", group)])),
    ]
}

#[tokio::test]
async fn an_engine_without_access_control_is_refused_with_the_capability_rule() {
    let emulator = Emulator::new();

    // A user and a group for the two MODIFY surfaces to name, so the engine rule
    // is what refuses the request rather than an unresolved id.
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "rule-user"),
                ("UserName", "rule-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateUser must answer 200, got {} {}",
        created.status, created.body
    );
    let mut group_request = params(&[("UserGroupId", "rule-group"), ("Engine", "redis")]);
    group_request.push(("UserIds.member.1".to_string(), "default".to_string()));
    let created_group = emulator.call("CreateUserGroup", &group_request).await;
    assert_eq!(
        created_group.status, 200,
        "CreateUserGroup must answer 200, got {} {}",
        created_group.status, created_group.body
    );

    // The axis: WHY access control does not exist for the engine named — a cache
    // engine the service does run but that has no ACLs (`memcached`), engines of
    // other services, a token no engine carries, the letter case of one of them,
    // and the empty value. `EngineType` is modelled `pattern ^[a-zA-Z]*$`
    // (`smithy_query(query="shape", shape="EngineType")`), so every row is
    // letters only: a row the boundary validator rejected would never reach the
    // rule under test.
    let without_access_control = ["memcached", "mysql", "notarealengine", "MEMCACHED", ""];
    for engine in without_access_control {
        for (action, pairs) in access_control_writers("rule-user", "rule-group") {
            let mut request = pairs;
            request.push(("Engine".to_string(), engine.to_string()));
            let reply = emulator.call(action, &request).await;
            refused(
                &format!("{action}(Engine={engine})"),
                &reply,
                400,
                "InvalidParameterValue",
                ACCESS_CONTROL_RULE,
            );
        }
    }

    // The engines access control DOES exist for are accepted, in any letter case
    // ("this value is stored as a lowercase string"), so the refusal above is
    // the capability rule speaking.
    for (index, engine) in ["valkey", "ReDiS"].into_iter().enumerate() {
        let accepted = emulator
            .call(
                "CreateUser",
                &params(&[
                    ("UserId", &format!("capable-{index}")),
                    ("UserName", &format!("capable-{index}")),
                    ("Engine", engine),
                    ("AccessString", "on ~* +@all"),
                    ("NoPasswordRequired", "true"),
                ]),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {engine}: CreateUser must answer 200, got {} {}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.text("Engine"),
            Some(engine.to_ascii_lowercase().as_str()),
            "case {engine}: the stored engine must be the lowercased one, got {}",
            accepted.body
        );
    }
}

#[tokio::test]
async fn an_engine_that_cannot_run_a_serverless_cache_states_the_capability_rule() {
    let emulator = Emulator::new();

    // A cache for the MODIFY surface to name, so the engine rule is what refuses
    // the request rather than an unresolved name.
    let created = emulator
        .call(
            "CreateServerlessCache",
            &params(&[("ServerlessCacheName", "rule-cache"), ("Engine", "redis")]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateServerlessCache must answer 200, got {} {}",
        created.status, created.body
    );

    // The axis: HOW the engine misses the serverless-capable set — an engine of
    // another service, a token no engine carries, a version written into the
    // name, the letter case of nothing published, and the empty value.
    let incapable = ["mysql", "not-a-real-engine", "redis7", "DynamoDB", ""];
    for (index, engine) in incapable.into_iter().enumerate() {
        let created = emulator
            .call(
                "CreateServerlessCache",
                &params(&[
                    ("ServerlessCacheName", &format!("serverless-rule-{index}")),
                    ("Engine", engine),
                ]),
            )
            .await;
        refused(
            &format!("CreateServerlessCache(Engine={engine})"),
            &created,
            400,
            "InvalidParameterValue",
            SERVERLESS_RULE,
        );
        // The same capability decides the engine a serverless cache may be moved
        // to, so the modify states the same rule.
        let modified = emulator
            .call(
                "ModifyServerlessCache",
                &params(&[("ServerlessCacheName", "rule-cache"), ("Engine", engine)]),
            )
            .await;
        refused(
            &format!("ModifyServerlessCache(Engine={engine})"),
            &modified,
            400,
            "InvalidParameterValue",
            SERVERLESS_RULE,
        );
    }

    // The engines a serverless cache DOES run — "The options are valkey,
    // memcached or redis" (`smithy_query(query="doc",
    // shape="ModifyServerlessCacheRequest", member="Engine")`) — are accepted in
    // any letter case.
    for (index, engine) in ["valkey", "memcached", "REDIS"].into_iter().enumerate() {
        let accepted = emulator
            .call(
                "CreateServerlessCache",
                &params(&[
                    ("ServerlessCacheName", &format!("capable-cache-{index}")),
                    ("Engine", engine),
                ]),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {engine}: CreateServerlessCache must answer 200, got {} {}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.text("ServerlessCache/Engine"),
            Some(engine.to_ascii_lowercase().as_str()),
            "case {engine}: the stored engine must be the lowercased one, got {}",
            accepted.body
        );
    }
}

#[tokio::test]
async fn a_resource_name_that_is_not_an_arn_states_the_structural_rule() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "tagged-cluster", &[("Engine", "redis")]).await;

    // The axis: the COMPONENT COUNT, which is the rule a value that is not
    // ARN-shaped breaks — one component, five, six — against the seven an
    // ElastiCache ARN carries.
    let wrong_component_count = [
        "not-an-arn".to_string(),
        String::new(),
        "arn:aws:s3:::a-bucket".to_string(),
        format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster"),
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/tagged-cluster"),
    ];
    for value in &wrong_component_count {
        assert_refused_by_every_tagging_operation(
            &emulator,
            value,
            ARN_COMPONENTS_RULE,
            value.split(':').count(),
        )
        .await;
    }

    // The axis: WHICH field of a seven-component value names no ElastiCache
    // resource — the prefix, the partition, the service, the Region, the
    // resource type and the resource name.
    let not_an_elasticache_resource = [
        format!("urn:aws:elasticache:{REGION}:{ACCOUNT}:cluster:tagged-cluster"),
        format!("arn::elasticache:{REGION}:{ACCOUNT}:cluster:tagged-cluster"),
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table:tagged-cluster"),
        format!("arn:aws:elasticache::{ACCOUNT}:cluster:tagged-cluster"),
        format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:widget:tagged-cluster"),
        format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster:"),
    ];
    for value in &not_an_elasticache_resource {
        assert_refused_by_every_tagging_operation(
            &emulator,
            value,
            ARN_RESOURCE_RULE,
            value.split(':').count(),
        )
        .await;
    }

    // The ARN of a resource the account DOES hold is served, so the refusals
    // above are the ARN rules speaking.
    let arn = format!("arn:aws:elasticache:{REGION}:{ACCOUNT}:cluster:tagged-cluster");
    let listed = list_tags(&emulator, &arn).await;
    assert_eq!(
        listed.status, 200,
        "a stored cluster's ARN must be served, got {} {}",
        listed.status, listed.body
    );
}

/// Assert all three tagging operations refuse `value` with 400 `InvalidARN` and
/// exactly `message`, and that the message carries no part of the value.
async fn assert_refused_by_every_tagging_operation(
    emulator: &Emulator,
    value: &str,
    message: &str,
    components: usize,
) {
    let case = format!("{value:?} ({components} components)");
    let replies = [
        (
            "AddTagsToResource",
            add_tags(emulator, value, &[("k", Some("v"))]).await,
        ),
        ("ListTagsForResource", list_tags(emulator, value).await),
        (
            "RemoveTagsFromResource",
            remove_tags(emulator, value, &["k"]).await,
        ),
    ];
    for (operation, reply) in &replies {
        refused(
            &format!("{operation} on {case}"),
            reply,
            400,
            "InvalidARN",
            message,
        );
        assert!(
            value.is_empty() || !reply.error_message().contains(value),
            "case {case}: {operation} must not echo the value, got {:?}",
            reply.error_message()
        );
    }
}

#[tokio::test]
async fn the_snapshot_creator_words_an_unresolved_cluster_as_the_service_words_it_there() {
    let emulator = Emulator::new();
    create_available_cluster(&emulator, "snapshot-source", &[("Engine", "redis")]).await;

    // The axis: the FORM of the identifier that resolves to no cluster — short,
    // long, one carrying digits — each a legal identifier, so the refusal is the
    // resolution's and not the grammar's.
    let absent = [
        "ghost",
        "a-cluster-that-was-never-created-at-all",
        "ghost-9-with-digits",
    ];
    for (index, id) in absent.into_iter().enumerate() {
        let snapshot = emulator
            .call(
                "CreateSnapshot",
                &params(&[
                    ("SnapshotName", &format!("ghost-snap-{index}")),
                    ("CacheClusterId", id),
                ]),
            )
            .await;
        refused(
            &format!("CreateSnapshot({id})"),
            &snapshot,
            404,
            "CacheClusterNotFound",
            &format!("Cache cluster not found: {id}"),
        );
        // The surfaces that only RESOLVE a `CacheClusterId` name the resource the
        // way the shape spells it, which is how the service words it there.
        for action in ["DescribeCacheClusters", "DeleteCacheCluster"] {
            let reply = emulator
                .call(action, &params(&[("CacheClusterId", id)])).await;
            refused(
                &format!("{action}({id})"),
                &reply,
                404,
                "CacheClusterNotFound",
                &format!("CacheCluster not found: {id}"),
            );
        }
    }

    // A snapshot of the cluster that IS there is taken, so the refusal above is
    // the resolution speaking.
    let taken = emulator
        .call(
            "CreateSnapshot",
            &params(&[
                ("SnapshotName", "real-snap"),
                ("CacheClusterId", "snapshot-source"),
            ]),
        )
        .await;
    assert_eq!(
        taken.status, 200,
        "CreateSnapshot of a stored cluster must answer 200, got {} {}",
        taken.status, taken.body
    );
}

/// The `Status` `DescribeUserGroups` reports for `group`.
async fn user_group_status(emulator: &Emulator, group: &str) -> String {
    let described = emulator
        .call("DescribeUserGroups", &params(&[("UserGroupId", group)]))
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeUserGroups must answer 200, got {} {}",
        described.status, described.body
    );
    described
        .text("Status")
        .unwrap_or_else(|| panic!("no Status in {}", described.body))
        .to_string()
}

/// Create one user group out of the service's own user, asserting it succeeded,
/// and answer with that call's own reply.
async fn create_user_group(emulator: &Emulator, group: &str) -> Reply {
    let mut request = params(&[("UserGroupId", group), ("Engine", "redis")]);
    request.push(("UserIds.member.1".to_string(), "default".to_string()));
    let created = emulator.call("CreateUserGroup", &request).await;
    assert_eq!(
        created.status, 200,
        "case {group}: CreateUserGroup must answer 200, got {} {}",
        created.status, created.body
    );
    created
}

/// The `Status` `reply` reports: the variable part of the sentence below, taken
/// from the answer of the call that put the group in that status rather than
/// from a literal written here.
fn answered_status(case: &str, reply: &Reply) -> String {
    reply
        .text("Status")
        .unwrap_or_else(|| panic!("case {case:?}: no Status in {}", reply.body))
        .to_string()
}

#[tokio::test]
async fn a_user_group_that_cannot_be_deleted_states_the_statuses_a_delete_admits() {
    let emulator = Emulator::new();
    // A second user for the modify below to ADD, so the change the clock has not
    // settled is a real membership change rather than a repeated member.
    let created_user = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", "extra-user"),
                ("UserName", "extra-user"),
                ("Engine", "redis"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created_user.status, 200,
        "CreateUser must answer 200, got {} {}",
        created_user.status, created_user.body
    );

    // The axis: the status the group is in when the delete arrives — one still
    // being created, one carrying a change the clock has not settled, and one
    // already on its way out. Each row reads the status out of the answer of the
    // call that PUT the group in it, so the message's variable part is the
    // group's own and not a literal, and the delete under test follows that
    // answer straight away: a report of the group is answered only once the span
    // has passed, so a read between the two would wait the group out of the
    // status the row is about.
    for (case, group) in [
        ("still creating", "creating-group"),
        ("still modifying", "modifying-group"),
        ("already deleting", "deleting-group"),
    ] {
        let created = create_user_group(&emulator, group).await;
        let status = match case {
            "still creating" => answered_status(case, &created),
            _ => {
                // Reading the group back is answered once the create has
                // settled it `active`, which is what lets the next change be
                // accepted.
                user_group_status(&emulator, group).await;
                let action = if case == "still modifying" {
                    "ModifyUserGroup"
                } else {
                    "DeleteUserGroup"
                };
                let mut request = params(&[("UserGroupId", group)]);
                if action == "ModifyUserGroup" {
                    request.push((
                        "UserIdsToAdd.member.1".to_string(),
                        "extra-user".to_string(),
                    ));
                }
                let accepted = emulator.call(action, &request).await;
                assert_eq!(
                    accepted.status, 200,
                    "case {case}: {action} must answer 200, got {} {}",
                    accepted.status, accepted.body
                );
                answered_status(case, &accepted)
            }
        };

        let sentence = format!(
            "User group {group} has status {status}. Only active or create-failed user groups \
             can be deleted."
        );
        let reply = emulator
            .call("DeleteUserGroup", &params(&[("UserGroupId", group)]))
            .await;
        refused(case, &reply, 400, "InvalidUserGroupState", &sentence);
        // A refused delete left the status alone: the identical call is refused
        // over the same status again.
        let again = emulator
            .call("DeleteUserGroup", &params(&[("UserGroupId", group)]))
            .await;
        refused(case, &again, 400, "InvalidUserGroupState", &sentence);
    }

    // The status a delete DOES admit: the group the clock has settled `active` is
    // deleted, which is what makes the refusals above the state rule speaking.
    create_user_group(&emulator, "deletable-group").await;
    assert_eq!(
        user_group_status(&emulator, "deletable-group").await,
        state::USER_STATUS_ACTIVE,
        "the read after the create must report the group active"
    );
    let deleted = emulator
        .call(
            "DeleteUserGroup",
            &params(&[("UserGroupId", "deletable-group")]),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "an active group must be deleted, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.text("Status"),
        Some(state::STATUS_DELETING),
        "the delete must answer with the group leaving, got {}",
        deleted.body
    );
}
