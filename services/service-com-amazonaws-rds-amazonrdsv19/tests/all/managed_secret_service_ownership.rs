// SPDX-License-Identifier: CC0-1.0

//! The secret RDS manages for a resource's master user password is a secret RDS OWNS.
//!
//! The RULE under test, asserted over a table of resources rather than one recorded call:
//! a secret an Amazon Web Services service creates and manages for one of its own
//! resources is reported as that service's, not as a secret of the account that happened to
//! ask for it. Three things follow from that, and each is asserted on every row:
//!
//! * the secret is NAMED under the prefix AWS reserves for the owning service
//!   (`crate`'s `state::MANAGED_SECRET_NAME_PREFIX`), a form no caller can mint, and then
//!   the kind of resource it belongs to and that resource's immutable resource id;
//! * `DescribeSecret` reports `OwningService` as that service's id — "The ID of the service
//!   that created this secret";
//! * the secret carries the AWS-applied tag naming the ARN of the resource whose master
//!   user password it holds (`aws:rds:primaryDBInstanceArn` for a DB instance,
//!   `aws:rds:primaryDBClusterArn` for a DB cluster), which is the reverse lookup from a
//!   credential back to the database it belongs to.
//!
//! The rows differ along what the rule generalises over: the kind of resource (DB instance,
//! DB cluster), the engine, and whether the management was turned on by the CREATE or by a
//! later `ModifyDBInstance` — the two paths that mint such a secret. The last row is the
//! other side of the rule: a secret a CALLER creates owns itself, reports no
//! `OwningService`, and cannot be named in the reserved form at all.
//!
//! Secrets Manager and KMS are mounted beside the RDS service over one engine
//! ([`crate::managed_master_user_password::emulator`]), because the secret read back here
//! is a real secret of this emulator.

use aws_sdk_secretsmanager::error::ProvideErrorMetadata;
use aws_sdk_secretsmanager::operation::create_secret::CreateSecretError;
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use service_com_amazonaws_secretsmanager_secretsmanager::state as sm;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, expect_ok, text, wait_for_settle, REGION};
use crate::managed_master_user_password::{
    emulator, held_credentials, master_user_secret, secrets_manager, the_resource, ACCOUNT,
};

/// Which resource a row's secret belongs to, and how that resource is addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Instance,
    Cluster,
}

impl Owner {
    /// The operation that creates it.
    fn create(self) -> &'static str {
        match self {
            Self::Instance => "CreateDBInstance",
            Self::Cluster => "CreateDBCluster",
        }
    }

    /// The member its identifier travels in.
    fn identifier_member(self) -> &'static str {
        match self {
            Self::Instance => "DBInstanceIdentifier",
            Self::Cluster => "DBClusterIdentifier",
        }
    }

    /// The element the resource is reported in.
    fn element(self) -> &'static str {
        match self {
            Self::Instance => "DBInstance",
            Self::Cluster => "DBCluster",
        }
    }

    /// The member carrying the resource's own ARN.
    fn arn_member(self) -> &'static str {
        match self {
            Self::Instance => "DBInstanceArn",
            Self::Cluster => "DBClusterArn",
        }
    }

    /// The AWS-applied tag key the secret of this kind of resource carries.
    fn owner_tag_key(self) -> &'static str {
        match self {
            Self::Instance => st::PRIMARY_DB_INSTANCE_ARN_TAG,
            Self::Cluster => st::PRIMARY_DB_CLUSTER_ARN_TAG,
        }
    }

    /// The part of the secret's name that follows the reserved prefix, for the kind of
    /// resource the secret belongs to.
    fn name_infix(self) -> &'static str {
        match self {
            Self::Instance => "db-",
            Self::Cluster => "cluster-",
        }
    }
}

/// One row: the resource whose managed secret is read back, and whether the management was
/// turned on by the create or by a later modification.
struct OwnedSecretCase {
    case: &'static str,
    owner: Owner,
    engine: &'static str,
    master_username: &'static str,
    extra: &'static [(&'static str, &'static str)],
    /// `true` for a row that creates the resource WITHOUT management and then turns it on
    /// with `ModifyDBInstance`, the other path that mints such a secret.
    managed_by_modification: bool,
}

const OWNED_SECRET_CASES: &[OwnedSecretCase] = &[
    OwnedSecretCase {
        case: "an RDS for MySQL DB instance managed from its create",
        owner: Owner::Instance,
        engine: "mysql",
        master_username: "admin",
        extra: &[("DBInstanceClass", "db.t3.micro"), ("AllocatedStorage", "20")],
        managed_by_modification: false,
    },
    OwnedSecretCase {
        case: "an RDS for PostgreSQL DB instance managed from its create",
        owner: Owner::Instance,
        engine: "postgres",
        master_username: "dbadmin",
        extra: &[("DBInstanceClass", "db.t4g.micro"), ("AllocatedStorage", "20")],
        managed_by_modification: false,
    },
    OwnedSecretCase {
        case: "an Aurora PostgreSQL DB cluster managed from its create",
        owner: Owner::Cluster,
        engine: "aurora-postgresql",
        master_username: "clusteradmin",
        extra: &[],
        managed_by_modification: false,
    },
    OwnedSecretCase {
        case: "an RDS for MariaDB DB instance managed by a later modification",
        owner: Owner::Instance,
        engine: "mariadb",
        master_username: "mariaadmin",
        extra: &[("DBInstanceClass", "db.t3.small"), ("AllocatedStorage", "20")],
        managed_by_modification: true,
    },
];

#[tokio::test]
async fn a_managed_master_user_secret_is_named_owned_and_tagged_by_rds() {
    let router = emulator();
    for (index, row) in OWNED_SECRET_CASES.iter().enumerate() {
        let identifier = format!("emu-owned-{index}");
        let mut params: Vec<(&str, String)> = vec![
            (row.owner.identifier_member(), identifier.clone()),
            ("Engine", row.engine.to_string()),
            ("MasterUsername", row.master_username.to_string()),
        ];
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        if row.managed_by_modification {
            // A resource created with a password of the caller's own, so the secret is
            // minted by the modification below rather than by the create.
            params.push(("MasterUserPassword", "a-password-of-its-own".to_string()));
        } else {
            params.push(("ManageMasterUserPassword", "true".to_string()));
        }
        let created = expect_ok(row.case, call(&router, row.owner.create(), &params).await);
        let mut envelope = the_resource(row.case, &created, row.owner.element());
        if row.managed_by_modification {
            // The instance must have reached `available` before it can be modified, which
            // is the transition the clock owes its create.
            wait_for_settle();
            let modified = expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBInstance",
                    &[
                        (row.owner.identifier_member(), identifier.as_str()),
                        ("ManageMasterUserPassword", "true"),
                        ("ApplyImmediately", "true"),
                    ],
                )
                .await,
            );
            envelope = the_resource(row.case, &modified, row.owner.element());
        }
        let resource_arn = text(&envelope, row.owner.arn_member());
        let secret = master_user_secret(row.case, &envelope).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the resource to report MasterUserSecret, got {envelope}",
                row.case
            )
        });
        let secret_arn = text(&secret, "SecretArn");

        // Read the secret back through Secrets Manager's own operations.
        let described = secrets_manager(&router)
            .describe_secret()
            .secret_id(&secret_arn)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {:?}: Secrets Manager holds no secret at {secret_arn}: {error}",
                    row.case
                )
            });

        // 1. The reserved name: `rds!<kind>-<resource id>`, carrying the resource's own
        //    immutable id, and a form no caller can mint.
        let name = described.name().unwrap_or_default().to_string();
        let expected_prefix = format!(
            "{}{}",
            st::MANAGED_SECRET_NAME_PREFIX,
            row.owner.name_infix()
        );
        assert!(
            name.starts_with(&expected_prefix),
            "case {:?}: expected the secret's name to start with {expected_prefix:?}, got \
             {name:?}",
            row.case
        );
        let resource_id = text(
            &envelope,
            match row.owner {
                Owner::Instance => "DbiResourceId",
                Owner::Cluster => "DbClusterResourceId",
            },
        );
        assert_eq!(
            name,
            format!("{expected_prefix}{resource_id}"),
            "case {:?}: expected the secret's name to carry the resource's own id \
             {resource_id:?}, got {name:?}",
            row.case
        );
        assert!(
            secret_arn.starts_with(&format!(
                "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{name}-"
            )),
            "case {:?}: expected the secret's ARN to carry that name, got {secret_arn:?}",
            row.case
        );

        // 2. The owning service: the id of the service that created the secret.
        assert_eq!(
            described.owning_service(),
            Some("rds"),
            "case {:?}: expected OwningService \"rds\" for a secret RDS manages, got {:?}",
            row.case,
            described.owning_service()
        );

        // 3. The AWS-applied tag naming the resource the credentials belong to.
        let tags: Vec<(&str, &str)> = described
            .tags()
            .iter()
            .map(|tag| (tag.key().unwrap_or_default(), tag.value().unwrap_or_default()))
            .collect();
        assert!(
            tags.contains(&(row.owner.owner_tag_key(), resource_arn.as_str())),
            "case {:?}: expected the tag {}={resource_arn} on the secret, got {tags:?}",
            row.case,
            row.owner.owner_tag_key()
        );

        // And the secret is still the credentials document a client reads the master user's
        // password out of: the ownership changes who owns it, not what it holds.
        let held = held_credentials(row.case, &router, &secret_arn).await;
        assert_eq!(
            held.username.as_deref(),
            Some(row.master_username),
            "case {:?}: expected the secret to hold the master user the request named, got \
             {:?}",
            row.case,
            held.username
        );
        assert_eq!(
            st::check_master_user_password(row.engine, &held.password),
            Ok(()),
            "case {:?}: the generated password must satisfy MasterUserPassword for engine {}",
            row.case,
            row.engine
        );
    }
}

/// The third minting site the rule covers: a TENANT DATABASE of a multi-tenant DB
/// instance. Its master user is a user of that INSTANCE, so the secret belongs to the
/// instance — the same reserved prefix, the same owning service, and the instance's own ARN
/// under the DB-instance tag key.
#[tokio::test]
async fn a_tenant_databases_managed_secret_is_owned_by_rds_too() {
    let router = emulator();
    let case = "a tenant database of an Oracle container database";
    let created_instance = expect_ok(
        case,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-owned-cdb"),
                ("Engine", "oracle-ee-cdb"),
                ("DBInstanceClass", "db.m5.large"),
                ("MasterUsername", "oracleadmin"),
                ("MasterUserPassword", "the-callers-own-password"),
                ("MultiTenant", "true"),
            ],
        )
        .await,
    );
    let instance = the_resource(case, &created_instance, "DBInstance");
    let instance_arn = text(&instance, "DBInstanceArn");
    let resource_id = text(&instance, "DbiResourceId");
    wait_for_settle();
    let created = expect_ok(
        case,
        call(
            &router,
            "CreateTenantDatabase",
            &[
                ("DBInstanceIdentifier", "emu-owned-cdb"),
                ("TenantDBName", "emutenant"),
                ("MasterUsername", "tenantadmin"),
                ("ManageMasterUserPassword", "true"),
            ],
        )
        .await,
    );
    let envelope = the_resource(case, &created, "TenantDatabase");
    let secret = master_user_secret(case, &envelope)
        .unwrap_or_else(|| panic!("case {case:?}: expected a MasterUserSecret: {envelope}"));
    let secret_arn = text(&secret, "SecretArn");
    let described = secrets_manager(&router)
        .describe_secret()
        .secret_id(&secret_arn)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Secrets Manager holds no secret at {secret_arn}: {error}")
        });
    assert_eq!(
        described.name(),
        Some(
            format!(
                "{}tenantdb-{resource_id}-emutenant",
                st::MANAGED_SECRET_NAME_PREFIX
            )
            .as_str()
        ),
        "case {case:?}: expected the reserved name carrying the instance's own id, got {:?}",
        described.name()
    );
    assert_eq!(
        described.owning_service(),
        Some("rds"),
        "case {case:?}: expected OwningService \"rds\", got {:?}",
        described.owning_service()
    );
    let tags: Vec<(&str, &str)> = described
        .tags()
        .iter()
        .map(|tag| (tag.key().unwrap_or_default(), tag.value().unwrap_or_default()))
        .collect();
    assert!(
        tags.contains(&(st::PRIMARY_DB_INSTANCE_ARN_TAG, instance_arn.as_str())),
        "case {case:?}: expected the tag {}={instance_arn} on the secret, got {tags:?}",
        st::PRIMARY_DB_INSTANCE_ARN_TAG
    );
}

/// The other side of the rule: a secret a CALLER creates is the caller's own — it reports
/// no `OwningService` — and the reserved name form is not a caller's to mint, whichever
/// service's prefix it claims.
#[tokio::test]
async fn a_callers_own_secret_is_unowned_and_cannot_claim_a_reserved_name() {
    let router = emulator();
    let created = secrets_manager(&router)
        .create_secret()
        .name("emu-caller-secret")
        .secret_string("{\"username\":\"someone\",\"password\":\"theirs\"}")
        .send()
        .await
        .expect("a caller creates a secret of its own");
    let arn = created.arn().expect("the create reports an ARN").to_string();
    let described = secrets_manager(&router)
        .describe_secret()
        .secret_id(&arn)
        .send()
        .await
        .expect("the caller's own secret is described");
    assert_eq!(
        described.owning_service(),
        None,
        "case a caller's own secret: expected no OwningService, got {:?}",
        described.owning_service()
    );

    // The reserved form is refused for every caller, for the prefix of the service that
    // owns such secrets and for any other service's prefix. A name no caller may use is a
    // value that failed the rules for that member, which Secrets Manager answers with its
    // constraint refusal and the one sentence it states a name's character set in
    // (`INVALID_SECRET_NAME_MESSAGE`) — `ValidationException` is not in `CreateSecret`'s
    // modelled errors, so the SDK surfaces it through the error metadata rather than as a
    // typed variant.
    for (case, name) in [
        ("the RDS prefix", "rds!db-db-ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ("another service's prefix", "events!connection/a/b"),
        ("a reserved name with no owner", "!db-ABCDEF"),
    ] {
        let refused = secrets_manager(&router)
            .create_secret()
            .name(name)
            .secret_string("{\"password\":\"theirs\"}")
            .send()
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the create of {name:?} to be refused")
            });
        let service_error = refused.into_service_error();
        assert_eq!(
            ProvideErrorMetadata::code(&service_error),
            Some("ValidationException"),
            "case {case:?}: expected the constraint refusal for the name {name:?}, got \
             {service_error:?}"
        );
        assert_eq!(
            ProvideErrorMetadata::message(&service_error),
            Some(sm::INVALID_SECRET_NAME_MESSAGE),
            "case {case:?}: expected the name sentence for {name:?}, got {service_error:?}"
        );
    }

    // And a caller may not write the AWS-applied tag either, on a name it may use.
    let tagged = secrets_manager(&router)
        .create_secret()
        .name("emu-caller-tagged")
        .secret_string("{\"password\":\"theirs\"}")
        .tags(
            aws_sdk_secretsmanager::types::Tag::builder()
                .key(st::PRIMARY_DB_INSTANCE_ARN_TAG)
                .value(format!(
                    "arn:aws:rds:{REGION}:{ACCOUNT}:db:not-this-callers-instance"
                ))
                .build(),
        )
        .send()
        .await
        .expect_err("case a caller's AWS-applied tag: expected the create to be refused");
    let service_error = tagged.into_service_error();
    assert!(
        matches!(
            service_error,
            CreateSecretError::InvalidParameterException(_)
        ),
        "case a caller's AWS-applied tag: expected InvalidParameterException, got \
         {service_error:?}"
    );
}

/// The managed secret of one resource is not the managed secret of another: each row above
/// names its own resource, so the names and the ARNs are distinct.
///
/// Mounted on its own router so it is independent of the table above.
#[tokio::test]
async fn every_resource_gets_a_secret_named_after_itself() {
    let router: MultiServiceRouter = emulator();
    let mut names: Vec<String> = Vec::new();
    for index in 0..3 {
        let identifier = format!("emu-own-name-{index}");
        let created = expect_ok(
            "a DB instance of its own",
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", "mysql"),
                    ("MasterUsername", "admin"),
                    ("ManageMasterUserPassword", "true"),
                    ("DBInstanceClass", "db.t3.micro"),
                    ("AllocatedStorage", "20"),
                ],
            )
            .await,
        );
        let envelope = the_resource("a DB instance of its own", &created, "DBInstance");
        let secret = master_user_secret("a DB instance of its own", &envelope)
            .expect("the instance reports its managed secret");
        let secret_arn = text(&secret, "SecretArn");
        let described = secrets_manager(&router)
            .describe_secret()
            .secret_id(&secret_arn)
            .send()
            .await
            .expect("Secrets Manager holds the secret");
        let name = described.name().unwrap_or_default().to_string();
        assert!(
            !names.contains(&name),
            "case instance {index}: expected a secret name of its own, but {name} was \
             already used"
        );
        names.push(name);
    }
}
