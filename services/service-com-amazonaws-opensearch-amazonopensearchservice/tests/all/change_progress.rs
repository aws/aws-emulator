// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDomainChangeProgress`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness, the baseline `CreateDomain` body and the table of
//! option containers come from [`super::domain_lifecycle`]; this suite adds no second
//! copy of them.
//!
//! The rule under test: a domain's configuration change is a RECORD written by the
//! call that starts it, addressed by its own `ChangeId`, reporting the options that
//! call set as pending while the change is in flight and as completed once the clock
//! has made it due. So the assertions span a SET of inputs — every option container
//! in the shared change table, change histories of one, two, three and six records,
//! the change addressed by id and the change left implicit, and both sides of the
//! provisioning span.

use serde_json::{json, Value};

use super::domain_lifecycle::{
    baseline_create, change_rows, create_baseline, error_code, string_member, Answer, Emulator,
    CHANGE_LOG_GROUP, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// A well-formed `GUID` that names no change: the id space is 128 bits wide and the
/// store mints v4 UUIDs, so the nil-shaped id below is never one of them.
const ABSENT_CHANGE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// `GET /2021-01-01/opensearch/domain/{DomainName}/progress`, with the `changeid`
/// query parameter when `change_id` is given.
async fn progress(emulator: &Emulator, name: &str, change_id: Option<&str>) -> Answer {
    let uri = match change_id {
        None => format!("{PREFIX}/opensearch/domain/{name}/progress"),
        Some(change_id) => {
            format!("{PREFIX}/opensearch/domain/{name}/progress?changeid={change_id}")
        }
    };
    emulator.call("GET", &uri, None).await
}

/// The `ChangeProgressStatus` of an accepted progress call.
async fn progress_status(emulator: &Emulator, name: &str, change_id: Option<&str>) -> Value {
    let answer = progress(emulator, name, change_id).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainChangeProgress({name:?}, {change_id:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ChangeProgressStatus"].clone()
}

/// The string members of a list member of a response body.
fn string_list(body: &Value, member: &str, case: &str) -> Vec<String> {
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a list {member}, got {body:?}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected strings in {member}, got {body:?}"))
                .to_owned()
        })
        .collect()
}

/// Assert `value` satisfies `GUID`'s own constraint: `@length(min: 36, max: 36)` with
/// `@pattern("^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$")`.
fn assert_guid(value: &str, case: &str) {
    assert_eq!(
        value.len(),
        36,
        "case {case}: a GUID is 36 characters, got {value:?} of {}",
        value.len()
    );
    let groups: Vec<&str> = value.split('-').collect();
    let widths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    assert_eq!(
        widths,
        vec![8, 4, 4, 4, 12],
        "case {case}: expected hex groups of 8-4-4-4-12, got {value:?} with {widths:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
        "case {case}: every GUID group is hexadecimal, got {value:?}"
    );
}

/// The option blocks a change applies PROPERTY BY PROPERTY, with the properties each is
/// applied as — stated here independently of the implementation, so a change in which
/// properties the service names is a change this suite sees.
pub(crate) const LEAF_PROPERTY_BLOCKS: [(&str, &[&str]); 3] = [
    (
        "AIMLOptions",
        &[
            "NaturalLanguageQueryGenerationOptions",
            "S3VectorsEngine",
            "ServerlessVectorAcceleration",
        ],
    ),
    (
        "AdvancedSecurityOptions",
        &[
            "AnonymousAuthDisableDate",
            "AnonymousAuthEnabled",
            "IAMFederationOptions",
            "InternalUserDatabaseEnabled",
            "JWTOptions",
            "MasterUserOptions",
            "SAMLOptions",
        ],
    ),
    (
        "ClusterConfig",
        &[
            "ColdStorageOptions",
            "DedicatedMasterCount",
            "DedicatedMasterEnabled",
            "DedicatedMasterType",
            "InstanceCount",
            "InstanceType",
            "MultiAZWithStandbyEnabled",
            "WarmCount",
            "WarmEnabled",
            "WarmStorage",
            "WarmType",
            "ZoneAwarenessEnabled",
        ],
    ),
];

/// The properties EVERY domain's configuration is made of, whatever the create named:
/// the service provisions a domain with a whole configuration, filling in the blocks the
/// request left out, so the change a create starts applies all of them — the blocks of
/// [`LEAF_PROPERTY_BLOCKS`] property by property, every other option whole, and the tag
/// set the create carried.
const PROVISIONED_PROPERTIES: [&str; 40] = [
    "AIMLOptions.NaturalLanguageQueryGenerationOptions",
    "AIMLOptions.S3VectorsEngine",
    "AIMLOptions.ServerlessVectorAcceleration",
    "AccessPolicies",
    "AdvancedOptions",
    "AdvancedSecurityOptions.AnonymousAuthDisableDate",
    "AdvancedSecurityOptions.AnonymousAuthEnabled",
    "AdvancedSecurityOptions.IAMFederationOptions",
    "AdvancedSecurityOptions.InternalUserDatabaseEnabled",
    "AdvancedSecurityOptions.JWTOptions",
    "AdvancedSecurityOptions.MasterUserOptions",
    "AdvancedSecurityOptions.SAMLOptions",
    "AutoTuneOptions",
    "AutomatedSnapshotPauseOptions",
    "ClusterConfig.ColdStorageOptions",
    "ClusterConfig.DedicatedMasterCount",
    "ClusterConfig.DedicatedMasterEnabled",
    "ClusterConfig.DedicatedMasterType",
    "ClusterConfig.InstanceCount",
    "ClusterConfig.InstanceType",
    "ClusterConfig.MultiAZWithStandbyEnabled",
    "ClusterConfig.WarmCount",
    "ClusterConfig.WarmEnabled",
    "ClusterConfig.WarmStorage",
    "ClusterConfig.WarmType",
    "ClusterConfig.ZoneAwarenessEnabled",
    "CognitoOptions",
    "DeploymentStrategyOptions",
    "DomainEndpointOptions",
    "EncryptionAtRestOptions",
    "EngineMode",
    "EngineVersion",
    "IPAddressType",
    "IdentityCenterOptions",
    "NodeToNodeEncryptionOptions",
    "OffPeakWindowOptions",
    "SnapshotOptions",
    "SoftwareUpdateOptions",
    "TAGS",
    "UseCase",
];

/// The property names setting `option` amounts to: a block the service applies property
/// by property ([`LEAF_PROPERTY_BLOCKS`]) names every one of its properties, and every
/// other option is applied whole.
fn leaf_names(option: &str, _value: &Value) -> Vec<String> {
    let Some((_, properties)) = LEAF_PROPERTY_BLOCKS
        .iter()
        .find(|(block, _)| *block == option)
    else {
        return vec![option.to_string()];
    };
    let mut names: Vec<String> = properties
        .iter()
        .map(|property| format!("{option}.{property}"))
        .collect();
    names.sort();
    names
}

/// The names of the properties a change reports, from the `<Member> : <value>` entries
/// it reports them as: the value each property is being set to is asserted by
/// [`super::domain_server_defaults`], and what this suite is about is WHICH properties
/// a change names.
fn property_names(properties: &[String]) -> Vec<String> {
    let mut names: Vec<String> = properties
        .iter()
        .map(|property| {
            property
                .split_once(" : ")
                .map_or(property.clone(), |(name, _)| name.to_owned())
        })
        .collect();
    names.sort();
    names
}

/// Every property the configuration a `CreateDomain` body provisions is made of: the
/// blocks the service fills in for every domain, the log-publishing configuration every
/// domain has, and the EBS volume, which a domain created without one does not have.
fn provisioned_properties(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = PROVISIONED_PROPERTIES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    if body.get("EBSOptions").is_some() {
        names.push("EBSOptions".to_string());
    }
    names.push("LogPublishingOptions".to_string());
    names.sort();
    names
}

/// The `DomainConfig` member names a [`baseline_create`] body sets: every property the
/// configuration the service provisions it with is made of. Derived from the body the
/// harness sends, so it cannot drift from it.
fn created_properties(name: &str) -> Vec<String> {
    provisioned_properties(&baseline_create(name))
}

#[tokio::test]
async fn a_change_reports_the_option_it_sets_pending_then_completed() {
    let emulator = Emulator::with_log_groups();
    // The log-publishing row of the shared table publishes to a log group, which has to
    // exist before an option enabling it is accepted.
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    // One row per kind of option container the shared change table spans: a cluster
    // structure, an EBS structure, a policy string, a free-form map, a snapshot
    // structure, an enum, a `LogType`-keyed map and a software-update structure. The
    // domains are created first so ONE wait settles every one of them: a domain that
    // is still being provisioned has work in flight, and the update under test is a
    // further change, which such a domain refuses.
    let rows = change_rows();
    for index in 0..rows.len() {
        create_baseline(&emulator, &format!("progress-{index}")).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for (index, (option, value, _)) in rows.into_iter().enumerate() {
        let name = format!("progress-{index}");
        let updated = emulator
            .update_config(&name, json!({ option: value.clone() }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case UpdateDomainConfig({option}): expected 200, got {} with {:?}",
            updated.status, updated.body
        );

        let in_flight = progress_status(&emulator, &name, None).await;
        let case = format!("DescribeDomainChangeProgress({option}, in flight)");
        let change_id = string_member(&in_flight, "ChangeId", &case);
        assert_guid(&change_id, &case);
        assert_eq!(
            (
                in_flight["Status"].as_str(),
                in_flight["ConfigChangeStatus"].as_str(),
                in_flight["InitiatedBy"].as_str(),
            ),
            (Some("PROCESSING"), Some("ApplyingChanges"), Some("CUSTOMER")),
            "case {case}: a change in flight is PROCESSING/ApplyingChanges, started by the \
             CUSTOMER, got {in_flight:?}"
        );
        assert_eq!(
            property_names(&string_list(&in_flight, "PendingProperties", &case)),
            leaf_names(option, &value),
            "case {case}: the option this change set is its one pending property, got {in_flight:?}"
        );
        assert!(
            string_list(&in_flight, "CompletedProperties", &case).is_empty(),
            "case {case}: nothing has completed yet, got {in_flight:?}"
        );
        let start_time = in_flight["StartTime"].clone();
        assert!(
            start_time.is_number() && in_flight["LastUpdatedTime"].is_number(),
            "case {case}: expected StartTime/LastUpdatedTime timestamps, got {in_flight:?}"
        );

        // A read does not move the record: the second answer is the first.
        let again = progress_status(&emulator, &name, None).await;
        assert_eq!(
            again, in_flight,
            "case {case}: a read must not change the change record: {in_flight:?} then {again:?}"
        );

        // The clock, not the reader, completes the change.
        tokio::time::sleep(SETTLE_WAIT).await;
        let settled = progress_status(&emulator, &name, None).await;
        let case = format!("DescribeDomainChangeProgress({option}, settled)");
        assert_eq!(
            (
                settled["ChangeId"].as_str(),
                settled["Status"].as_str(),
                settled["ConfigChangeStatus"].as_str(),
            ),
            (
                Some(change_id.as_str()),
                Some("COMPLETED"),
                Some("Completed")
            ),
            "case {case}: the same change is now COMPLETED/Completed, got {settled:?}"
        );
        assert_eq!(
            property_names(&string_list(&settled, "CompletedProperties", &case)),
            leaf_names(option, &value),
            "case {case}: the option this change set has completed, got {settled:?}"
        );
        assert!(
            string_list(&settled, "PendingProperties", &case).is_empty(),
            "case {case}: nothing is pending once the change has landed, got {settled:?}"
        );
        assert_eq!(
            settled["StartTime"], start_time,
            "case {case}: the start of a change does not move, got {:?} after {start_time:?}",
            settled["StartTime"]
        );
    }
}

#[tokio::test]
async fn the_change_a_create_starts_reports_every_option_the_create_set() {
    let emulator = Emulator::new();
    // The rule holds whatever the create carried, so the names span three bodies:
    // the baseline with every container, and two created with fewer members.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "full-create",
            baseline_create("full-create"),
            created_properties("full-create"),
        ),
        (
            "bare-create",
            json!({ "DomainName": "bare-create" }),
            // A create that names nothing still provisions a whole configuration: every
            // block the service fills in, with no EBS volume and an empty
            // log-publishing configuration.
            provisioned_properties(&json!({ "DomainName": "bare-create" })),
        ),
        (
            "one-option-create",
            json!({
                "DomainName": "one-option-create",
                "EngineVersion": "Elasticsearch_7.10",
                "AccessPolicies": "{\"Version\":\"2012-10-17\"}",
            }),
            provisioned_properties(&json!({
                "DomainName": "one-option-create",
                "EngineVersion": "Elasticsearch_7.10",
                "AccessPolicies": "{\"Version\":\"2012-10-17\"}",
            })),
        ),
    ];

    for (name, body, expected) in rows {
        let created = emulator.create(body).await;
        assert_eq!(
            created.status, 200,
            "case CreateDomain({name:?}): expected 200, got {} with {:?}",
            created.status, created.body
        );

        let status = progress_status(&emulator, name, None).await;
        let case = format!("DescribeDomainChangeProgress({name:?}, create)");
        assert_eq!(
            property_names(&string_list(&status, "PendingProperties", &case)),
            expected,
            "case {case}: expected the options the create set, got {status:?}"
        );
        assert_eq!(
            status["Status"].as_str(),
            Some("PENDING"),
            "case {case}: the change a create starts waits for the cluster to be provisioned, \
             got {status:?}"
        );
    }
}

#[tokio::test]
async fn every_change_keeps_its_own_id_and_the_latest_is_the_implicit_one() {
    let emulator = Emulator::new();
    // Spans the length of a domain's change history: the create alone (one change),
    // then one, two and five updates on top of it (two, three and six changes).
    for updates in [0usize, 1, 2, 5] {
        let name = format!("history-{updates}");
        create_baseline(&emulator, &name).await;
        // The service applies one change of a domain at a time, so each change has to
        // land before the next can be started: the waits below are what let this row
        // build a HISTORY rather than a queue.
        tokio::time::sleep(SETTLE_WAIT).await;

        // (change id, the properties that change set) in the order they were started.
        let mut history: Vec<(String, Vec<String>)> = vec![(
            string_member(
                &progress_status(&emulator, &name, None).await,
                "ChangeId",
                &format!("history-{updates}: create"),
            ),
            created_properties(&name),
        )];

        for (option, value, _) in change_rows().into_iter().take(updates) {
            let answer = emulator
                .update_config(&name, json!({ option: value.clone() }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case UpdateDomainConfig({name:?}, {option}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let status = progress_status(&emulator, &name, None).await;
            history.push((
                string_member(&status, "ChangeId", &format!("{name}: {option}")),
                leaf_names(option, &value),
            ));
            tokio::time::sleep(SETTLE_WAIT).await;
        }

        let ids: Vec<&String> = history.iter().map(|(id, _)| id).collect();
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {name}: every change has its own id, got {ids:?}"
        );

        // Each change stays addressable by its own id, reporting the properties it
        // set and not another change's.
        for (change_id, properties) in &history {
            let status = progress_status(&emulator, &name, Some(change_id)).await;
            let case = format!("{name}: by id {change_id}");
            assert_eq!(
                status["ChangeId"].as_str(),
                Some(change_id.as_str()),
                "case {case}: expected the change that was asked for, got {status:?}"
            );
            let reported: Vec<String> = property_names(
                &string_list(&status, "PendingProperties", &case)
                    .into_iter()
                    .chain(string_list(&status, "CompletedProperties", &case))
                    .collect::<Vec<String>>(),
            );
            assert_eq!(
                reported, *properties,
                "case {case}: expected the properties this change set, got {status:?}"
            );
        }

        // "If omitted, the request returns information about the most recent
        // configuration change."
        let implicit = progress_status(&emulator, &name, None).await;
        let (latest, _) = history
            .last()
            .expect("the create alone leaves one change in the history");
        assert_eq!(
            implicit["ChangeId"].as_str(),
            Some(latest.as_str()),
            "case {name}: a call naming no ChangeId reports the most recent change, got {implicit:?}"
        );
    }
}

#[tokio::test]
async fn an_absent_domain_is_the_generic_envelope_and_an_absent_change_the_modelled_not_found() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "present-domain").await;
    let known = string_member(
        &progress_status(&emulator, "present-domain", None).await,
        "ChangeId",
        "present-domain",
    );

    // Rows: a domain that was never created, a domain that was deleted, a well-formed
    // change id no change carries, and another domain's change id.
    create_baseline(&emulator, "other-domain").await;
    let other = string_member(
        &progress_status(&emulator, "other-domain", None).await,
        "ChangeId",
        "other-domain",
    );
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/other-domain"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(other-domain): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Rows: a domain that was never created, a domain that was deleted, a well-formed
    // change id no change carries, and another domain's change id — with the refusal each
    // earns. A name this account holds no domain under resolves no progress to report at
    // all, which is the generic envelope; a domain that IS held and carries no such change
    // is the modelled not-found.
    for (name, change_id, status, code) in [
        ("absent-domain", None, 400, "BaseException"),
        (
            "absent-domain",
            Some(ABSENT_CHANGE_ID),
            400,
            "BaseException",
        ),
        ("other-domain", None, 400, "BaseException"),
        (
            "present-domain",
            Some(ABSENT_CHANGE_ID),
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        ),
        (
            "present-domain",
            Some(other.as_str()),
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        ),
    ] {
        let answer = progress(&emulator, name, change_id).await;
        assert_eq!(
            answer.status, status,
            "case DescribeDomainChangeProgress({name:?}, {change_id:?}): expected {status}, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case DescribeDomainChangeProgress({name:?}, {change_id:?}): expected {code}, got \
             {:?}",
            answer.body
        );
    }

    // The change that does exist is still reported: a refusal for one id does not
    // disturb the record.
    let status = progress_status(&emulator, "present-domain", Some(&known)).await;
    assert_eq!(
        status["ChangeId"].as_str(),
        Some(known.as_str()),
        "case DescribeDomainChangeProgress(present-domain, known): expected the stored change, \
         got {status:?}"
    );
}

#[tokio::test]
async fn every_descriptor_of_a_domain_names_the_same_change() {
    let emulator = Emulator::new();
    let name = "descriptor-domain";
    let created = create_baseline(&emulator, name).await;

    // Both sides of the provisioning span, and after a second change: the change a
    // full descriptor carries is always the one the progress call reports.
    for stage in ["create", "create settled", "update"] {
        if stage == "create settled" {
            tokio::time::sleep(SETTLE_WAIT).await;
        }
        if stage == "update" {
            let answer = emulator
                .update_config(name, json!({ "SnapshotOptions": { "AutomatedSnapshotStartHour": 7 } }))
                .await;
            assert_eq!(
                answer.status, 200,
                "case UpdateDomainConfig({name:?}): expected 200, got {} with {:?}",
                answer.status, answer.body
            );
        }

        let progress = progress_status(&emulator, name, None).await;
        let described = emulator.describe(name).await;
        let configured = emulator.describe_config(name).await;
        let descriptors = [
            ("DescribeDomain", &described.body["DomainStatus"]["ChangeProgressDetails"]),
            (
                "DescribeDomainConfig",
                &configured.body["DomainConfig"]["ChangeProgressDetails"],
            ),
        ];
        for (reader, details) in descriptors {
            let case = format!("{reader}({name:?}, {stage})");
            for member in [
                "ChangeId",
                "ConfigChangeStatus",
                "InitiatedBy",
                "StartTime",
                "LastUpdatedTime",
            ] {
                assert_eq!(
                    details[member], progress[member],
                    "case {case}: {member} must be the change DescribeDomainChangeProgress \
                     reports ({:?}), got {:?}",
                    progress[member], details[member]
                );
            }
        }
    }

    // The create answer named a change too, and it is the one that was stored.
    assert_guid(
        &string_member(
            &created["ChangeProgressDetails"],
            "ChangeId",
            "CreateDomain(ChangeProgressDetails)",
        ),
        "CreateDomain(ChangeProgressDetails)",
    );
}

#[tokio::test]
async fn a_malformed_change_id_is_refused_by_the_modelled_constraint() {
    let emulator = Emulator::new();
    let name = "constrained-domain";
    create_baseline(&emulator, name).await;

    // `GUID` is `@length(min: 36, max: 36)` with the 8-4-4-4-12 hex pattern: each
    // published bound and one past it, plus one row per character class the pattern
    // refuses.
    for change_id in [
        // 35 characters: one short of the bound.
        "00000000-0000-4000-8000-00000000000",
        // 37 characters: one past it.
        "00000000-0000-4000-8000-0000000000000",
        // 36 characters, but not hexadecimal.
        "zzzzzzzz-0000-4000-8000-000000000000",
        // 36 characters, hexadecimal, but not grouped 8-4-4-4-12.
        "000000000-000-4000-8000-000000000000",
    ] {
        let answer = progress(&emulator, name, Some(change_id)).await;
        assert_eq!(
            answer.status, 400,
            "case DescribeDomainChangeProgress(changeid={change_id:?}): expected 400, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case DescribeDomainChangeProgress(changeid={change_id:?}): expected \
             ValidationException, got {:?}",
            answer.body
        );
    }

    // The bound itself is accepted: 36 hexadecimal characters in the published
    // groups reach the handler, which answers for the change they name.
    let answer = progress(&emulator, name, Some(ABSENT_CHANGE_ID)).await;
    assert_eq!(
        answer.status, RECORDED_NOT_FOUND_STATUS,
        "case DescribeDomainChangeProgress(changeid={ABSENT_CHANGE_ID:?}): a well-formed id \
         reaches the handler, got {} with {:?}",
        answer.status, answer.body
    );
}
