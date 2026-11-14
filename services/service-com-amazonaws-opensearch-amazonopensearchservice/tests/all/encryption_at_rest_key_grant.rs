// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS grant a domain's storage holds on the key its data is
//! encrypted under.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The keys are created and aliased through the AWS KMS service's OWN operations and
//! the grants are read back through its OWN `ListGrants`, both mounted on the same shared
//! engine ([`Emulator::with_kms`]): the grant under test is the one that service reports,
//! not a record this crate shows itself.
//!
//! THE RULE: a domain whose encryption at rest is ENABLED holds exactly one KMS grant on
//! the key its data is encrypted under — for this service's own regional service principal
//! as both grantee and retiring principal, issued by the account that owns the key,
//! permitting the operations the storage uses the key with (at least asking the key for a
//! data key and asking it to decrypt one back), and constrained by an encryption-context
//! subset naming that domain — and a domain that is not encrypted at rest, or no longer
//! exists, holds none.
//!
//! The rows differ along what the rule generalises over: which of the four spellings of a
//! key the caller writes (key ARN, bare key id, alias name, alias ARN), WHICH key the domain
//! is encrypted under (two different customer managed keys, and the Amazon Web Services
//! managed key of this service for a configuration that names none), and whether the
//! configuration enables encryption at all (disabled with a key named, and no container at
//! all). The lifecycle test then drives the rule over the calls that MOVE it: a second
//! domain on the same key, a change that re-states the encryption a domain already has, a
//! change that enables encryption on a domain created without it, and the delete that takes
//! the domain away.

use serde_json::{json, Value};

use super::domain_lifecycle::{baseline_create, settled_domain, Emulator, REGION};

/// The encryption-context key the data keys of ONE OpenSearch domain carry, and so the key
/// of the `EncryptionContextSubset` constraint the grant is scoped by: the domain's ARN
/// under `domainARN`, as the recorded KMS wire reports the constraint of such a grant.
const DOMAIN_ARN_CONTEXT: &str = "domainARN";

/// The two operations an encrypted-at-rest domain cannot do without, whatever else a grant
/// carries: envelope encryption of its storage asks the key for a data key
/// (`GenerateDataKey`) and asks the key to decrypt that data key back (`Decrypt`). Asserted
/// as a FLOOR, so a grant that permits more than this still satisfies the rule.
const REQUIRED_OPERATIONS: [&str; 2] = ["Decrypt", "GenerateDataKey"];

/// The alias a row points at a created key through KMS's own `CreateAlias`.
const KEY_ALIAS: &str = "alias/opensearch-grant-key";

/// The PREDEFINED Amazon Web Services alias of this service: `aws/<service-name>` is the
/// form of such an alias and `es` is this service's own name, so it names the Amazon Web
/// Services managed key a domain's data is encrypted under when the configuration names no
/// key of its own.
const AWS_MANAGED_ALIAS: &str = "alias/aws/es";

/// Which key a row's grant must be on.
#[derive(Debug, Clone)]
enum Expected {
    /// That key ARN, named here because the row created the key.
    Key(String),
    /// The Amazon Web Services managed key of this service, whose ARN is read from KMS at
    /// assert time: the alias names no key until the first call that needs one.
    ManagedKey,
    /// No grant anywhere: the domain's data is not encrypted at rest.
    NoGrant,
}

/// One row: the case it stands for, the `EncryptionAtRestOptions` to create the domain
/// with (`None` sends no container at all), and the key the grant must be on.
struct Row {
    case: &'static str,
    options: Option<Value>,
    expected: Expected,
}

/// An `EncryptionAtRestOptions` enabling encryption under `key`, however the row spells it.
fn enabled(key: &str) -> Value {
    json!({ "Enabled": true, "KmsKeyId": key })
}

/// The key id of a key ARN — everything after `key/`.
fn key_id_of(arn: &str) -> String {
    arn.rsplit_once("key/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| panic!("a KMS key ARN names the key after \"key/\", got {arn:?}"))
}

/// Create the alias [`KEY_ALIAS`] against `key_id` through KMS's own `CreateAlias`, and
/// answer its ARN — the fourth spelling of a key a configuration can carry.
async fn created_alias(emulator: &Emulator, key_id: &str) -> String {
    let answer = emulator
        .kms_call(
            REGION,
            "CreateAlias",
            json!({ "AliasName": KEY_ALIAS, "TargetKeyId": key_id }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateAlias({KEY_ALIAS:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    service_com_amazonaws_kms_trentservice::state::alias_arn(REGION, KEY_ALIAS)
}

/// The ARN of the Amazon Web Services managed key of this service, read through KMS's own
/// `DescribeKey` — which also asserts that the key behind the predefined alias is a key KMS
/// manages rather than one a caller created.
async fn managed_key_arn(emulator: &Emulator, case: &str) -> String {
    let answer = emulator
        .kms_call(REGION, "DescribeKey", json!({ "KeyId": AWS_MANAGED_ALIAS }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeKey({AWS_MANAGED_ALIAS:?}) to answer 200, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["KeyMetadata"]["KeyManager"],
        json!("AWS"),
        "case {case}: expected {AWS_MANAGED_ALIAS:?} to name an Amazon Web Services managed \
         key, got {:?}",
        answer.body["KeyMetadata"]
    );
    answer.body["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a key ARN, got {:?}", answer.body))
        .to_string()
}

/// Every grant KMS holds for `key`, read through its own `ListGrants` and paged to the end
/// the way a client reads them.
async fn grants_on(emulator: &Emulator, key: &str, case: &str) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut body = json!({ "KeyId": key });
        if let Some(marker) = marker.as_deref() {
            body["Marker"] = json!(marker);
        }
        let answer = emulator.kms_call(REGION, "ListGrants", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected ListGrants({key:?}) to answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        listed.extend(
            answer.body["Grants"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        marker = match answer.body["Truncated"].as_bool() {
            Some(true) => answer.body["NextMarker"].as_str().map(str::to_string),
            _ => None,
        };
        if marker.is_none() {
            return listed;
        }
    }
}

/// The grants among `listed` that are constrained to the domain `domain_arn` — the only
/// observable binding of that domain's storage to the key, which is what a client looking
/// for its domain's dependency on a key filters by.
fn for_domain(listed: &[Value], domain_arn: &str) -> Vec<Value> {
    listed
        .iter()
        .filter(|grant| {
            grant["Constraints"]["EncryptionContextSubset"][DOMAIN_ARN_CONTEXT]
                == json!(domain_arn)
        })
        .cloned()
        .collect()
}

/// The regional service principal the grant is for: this service, in the Region the domain
/// and its key both live in.
fn service_principal() -> String {
    format!("es.{REGION}.amazonaws.com")
}

/// Assert that `grant` is the grant an encrypted domain holds: whose it is, who may retire
/// it, which account issued it, which key it is on, what it permits and which data it
/// permits it for.
fn assert_domain_grant(grant: &Value, key_arn: &str, domain_arn: &str, case: &str) {
    let principal = service_principal();
    assert_eq!(
        grant["GranteePrincipal"],
        json!(principal),
        "case {case}: expected the grant to be for {principal:?}, got {grant:?}"
    );
    assert_eq!(
        grant["RetiringPrincipal"],
        json!(principal),
        "case {case}: expected {principal:?} to be able to retire the grant, got {grant:?}"
    );
    assert_eq!(
        grant["KeyId"],
        json!(key_arn),
        "case {case}: expected the grant to be on {key_arn:?}, got {grant:?}"
    );
    assert_eq!(
        grant["IssuingAccount"],
        json!(format!("arn:aws:iam::{}:root", ::engine::emulator_account_id())),
        "case {case}: expected the grant to be issued by the account that owns the key, got \
         {grant:?}"
    );
    assert_eq!(
        grant["Constraints"]["EncryptionContextSubset"],
        json!({ DOMAIN_ARN_CONTEXT: domain_arn }),
        "case {case}: expected the grant to be constrained to the domain's data, got {grant:?}"
    );

    let operations: Vec<String> = grant["Operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected the grant's Operations, got {grant:?}"))
        .iter()
        .map(|operation| {
            operation
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a GrantOperation, got {grant:?}"))
                .to_string()
        })
        .collect();
    for required in REQUIRED_OPERATIONS {
        assert!(
            operations.iter().any(|held| held == required),
            "case {case}: expected the grant to permit {required}, got {operations:?}"
        );
    }
    // Every operation a grant carries is a value of the KMS model's own `GrantOperation`
    // enum (`service_com_amazonaws_kms_trentservice::ops::types::GRANT_OPERATION_VALUES`),
    // so no permission outside that set is recorded.
    for operation in &operations {
        assert!(
            service_com_amazonaws_kms_trentservice::ops::types::GRANT_OPERATION_VALUES
                .contains(&operation.as_str()),
            "case {case}: {operation:?} is no GrantOperation, got {operations:?}"
        );
    }

    // A grant id is KMS's own: 64 lowercase hex characters, minted per grant — the form
    // that service reads a grant id by
    // (`service_com_amazonaws_kms_trentservice::state::names_a_grant_id`).
    let grant_id = grant["GrantId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected the grant's GrantId, got {grant:?}"));
    assert!(
        service_com_amazonaws_kms_trentservice::state::names_a_grant_id(grant_id),
        "case {case}: expected a KMS grant id, got {grant_id:?}"
    );
}

/// Create the domain `name` with the encryption `options` (none sends no container), and
/// answer the ARN it reports for itself — the value the grant's constraint has to name.
async fn created_domain(emulator: &Emulator, name: &str, options: Option<&Value>) -> String {
    let mut body = baseline_create(name);
    if let Some(options) = options {
        body["EncryptionAtRestOptions"] = options.clone();
    }
    let answer = emulator.create(body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateDomain({name:?}): expected the domain's ARN, got {:?}",
                answer.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn an_encrypted_domain_holds_one_kms_grant_on_the_key_its_data_is_encrypted_under() {
    let emulator = Emulator::with_kms();
    let first = emulator.created_key(REGION).await;
    let second = emulator.created_key(REGION).await;
    let alias_arn = created_alias(&emulator, &key_id_of(&first)).await;

    let rows = vec![
        Row {
            case: "the key named by its ARN",
            options: Some(enabled(&first)),
            expected: Expected::Key(first.clone()),
        },
        Row {
            case: "the same key named by its bare key id",
            options: Some(enabled(&key_id_of(&first))),
            expected: Expected::Key(first.clone()),
        },
        Row {
            case: "the same key named by an alias",
            options: Some(enabled(KEY_ALIAS)),
            expected: Expected::Key(first.clone()),
        },
        Row {
            case: "the same key named by that alias's ARN",
            options: Some(enabled(&alias_arn)),
            expected: Expected::Key(first.clone()),
        },
        Row {
            case: "another key entirely",
            options: Some(enabled(&second)),
            expected: Expected::Key(second.clone()),
        },
        Row {
            case: "encryption enabled naming no key of its own",
            options: Some(json!({ "Enabled": true })),
            expected: Expected::ManagedKey,
        },
        Row {
            case: "encryption disabled, with a key named beside it",
            options: Some(json!({ "Enabled": false, "KmsKeyId": first.clone() })),
            expected: Expected::NoGrant,
        },
        Row {
            case: "no encryption container at all",
            options: None,
            expected: Expected::NoGrant,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            options,
            expected,
        } = row;
        let name = format!("grant-domain-{index}");
        let domain_arn = created_domain(&emulator, &name, options.as_ref()).await;

        // The key the row expects the grant on, if any.
        let expected_key = match &expected {
            Expected::Key(arn) => Some(arn.clone()),
            Expected::ManagedKey => Some(managed_key_arn(&emulator, case).await),
            Expected::NoGrant => None,
        };

        // Every key under test, so a row asserts not only where its grant IS but that the
        // domain holds none anywhere else.
        let mut keys = vec![first.clone(), second.clone()];
        if let Some(expected_key) = expected_key.as_ref() {
            if !keys.contains(expected_key) {
                keys.push(expected_key.clone());
            }
        }
        for key in &keys {
            let held = for_domain(&grants_on(&emulator, key, case).await, &domain_arn);
            if expected_key.as_deref() == Some(key.as_str()) {
                assert_eq!(
                    held.len(),
                    1,
                    "case {case}: expected exactly one grant for {domain_arn} on {key}, got \
                     {held:?}"
                );
                assert_domain_grant(&held[0], key, &domain_arn, case);
            } else {
                assert!(
                    held.is_empty(),
                    "case {case}: expected no grant for {domain_arn} on {key}, got {held:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_grant_follows_the_domains_encryption_and_is_retired_with_the_domain() {
    let emulator = Emulator::with_kms();
    let first = emulator.created_key(REGION).await;
    let second = emulator.created_key(REGION).await;

    // Two domains encrypted under the SAME key: the key carries one grant per domain, each
    // constrained to its own domain, so a client reads off a key exactly which domains
    // depend on it.
    let shared = enabled(&first);
    let one = created_domain(&emulator, "grant-shared-one", Some(&shared)).await;
    let two = created_domain(&emulator, "grant-shared-two", Some(&shared)).await;
    for (case, domain_arn) in [("the first domain", &one), ("the second domain", &two)] {
        let held = for_domain(&grants_on(&emulator, &first, case).await, domain_arn);
        assert_eq!(
            held.len(),
            1,
            "case {case}: expected exactly one grant for {domain_arn} on the shared key, got \
             {held:?}"
        );
        assert_domain_grant(&held[0], &first, domain_arn, case);
    }

    // A change that re-states the encryption the domain already has takes no second grant:
    // the domain's dependency on the key is one fact however many calls state it. The
    // domain's own provisioning lands first — a domain with work in flight refuses a
    // further change.
    settled_domain(&emulator, "grant-shared-one").await;
    let answer = emulator
        .update_config(
            "grant-shared-one",
            json!({ "EncryptionAtRestOptions": shared.clone() }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateDomainConfig(re-stating the same encryption): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let held = for_domain(
        &grants_on(&emulator, &first, "re-stating the same encryption").await,
        &one,
    );
    assert_eq!(
        held.len(),
        1,
        "case re-stating the same encryption: expected the one grant to stand, got {held:?}"
    );

    // A domain created WITHOUT encryption at rest holds no grant, and the change that
    // enables it takes one — on the key that change names, not on the other key.
    let later = created_domain(&emulator, "grant-enabled-later", None).await;
    for key in [&first, &second] {
        let held = for_domain(&grants_on(&emulator, key, "before the change").await, &later);
        assert!(
            held.is_empty(),
            "case before the change: expected no grant for {later} on {key}, got {held:?}"
        );
    }
    settled_domain(&emulator, "grant-enabled-later").await;
    let answer = emulator
        .update_config(
            "grant-enabled-later",
            json!({ "EncryptionAtRestOptions": enabled(&second) }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case UpdateDomainConfig(enabling encryption at rest): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let case = "after enabling encryption at rest";
    let held = for_domain(&grants_on(&emulator, &second, case).await, &later);
    assert_eq!(
        held.len(),
        1,
        "case {case}: expected exactly one grant for {later} on {second}, got {held:?}"
    );
    assert_domain_grant(&held[0], &second, &later, case);
    let elsewhere = for_domain(&grants_on(&emulator, &first, case).await, &later);
    assert!(
        elsewhere.is_empty(),
        "case {case}: expected no grant for {later} on the key the change did not name, got \
         {elsewhere:?}"
    );

    // The domain's storage goes with the domain, so its grant is retired — and only its
    // own: the other domain on the same key keeps the permission its storage still needs.
    let answer = emulator.delete("grant-shared-one").await;
    assert_eq!(
        answer.status, 200,
        "case DeleteDomain(\"grant-shared-one\"): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let case = "after the delete";
    let listed = grants_on(&emulator, &first, case).await;
    assert!(
        for_domain(&listed, &one).is_empty(),
        "case {case}: expected the deleted domain's grant to be retired, got {listed:?}"
    );
    assert_eq!(
        for_domain(&listed, &two).len(),
        1,
        "case {case}: expected the surviving domain to keep its grant, got {listed:?}"
    );
}
