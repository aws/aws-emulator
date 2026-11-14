// SPDX-License-Identifier: CC0-1.0

//! The values the agent FETCHES for a container before that container exists — a container
//! definition's `secrets` — and the stop a task whose execution role may not read one earns.
//!
//! # The rule this module implements
//!
//! `ContainerDefinition$secrets` is "the secrets to pass to the container", each entry a
//! `Secret { name, valueFrom }` where `valueFrom` is "the secret to expose to the container.
//! The supported values are either the full ARN of the Secrets Manager secret or the full
//! ARN of the parameter in the SSM Parameter Store". The value is read BEFORE the container
//! starts and is exposed to it as the environment variable the entry's `name` spells, and
//! the read is made as the task's EXECUTION role —
//! `TaskDefinition$executionRoleArn` is "the task execution role that grants the Amazon ECS
//! container agent permission to make Amazon Web Services API calls on your behalf" — never
//! as the task role, which is the role the container's own calls are signed with.
//!
//! So a launch whose execution role may not read a secret the revision references launches
//! nothing: no container starts, nothing is delivered to the log group its configuration
//! names, and the task is stopped at the instant it would otherwise have started, with the
//! stop code of a task that never ran ([`STOP_CODE_TASK_FAILED_TO_START`]) and the
//! `stoppedReason` the agent words for a resource it could not retrieve
//! ([`unable_to_retrieve_secret_message`]). A launch whose role MAY read it runs, and each
//! entry's value is in the environment of the container's program
//! ([`super::container_runtime`]).
//!
//! Both halves ask the service that OWNS the secret: whether the role may read it, and
//! whether there is a value to read, are answered by Secrets Manager's own gate over its own
//! store ([`::service_com_amazonaws_secretsmanager_secretsmanager::state::secret_retrieval`]),
//! and the VALUE is answered by that service's own `GetSecretValue` over the emulator's
//! in-process callback — so what is injected is what a client reads back from that service,
//! and nothing is fetched for a reference it would refuse.
//!
//! A reference this module decides nothing about is launched: a `valueFrom` naming a Systems
//! Manager parameter is that service's to answer and is not decided here, and neither is one
//! that is no ARN at all.
//!
//! # Where each half happens
//!
//! The DECISION is taken at launch, in the pass that fetches the images the task's containers
//! run ([`evaluate_image_pulls`]): the agent retrieves the execution resources — the registry
//! authentication and the secrets — in one step before any container of the task starts,
//! which is why the two verdicts are recorded together and why the reason a refusal carries
//! names them together.
//!
//! The VALUE is read where the container's program starts, by the run that performs it
//! ([`super::container_runtime`]): that read is a call to another emulated service, which a
//! store pass cannot make. A secret that cannot be read THERE — one deleted between the
//! decision and the start, a `json-key` the value does not state — starts no program and
//! records no exit code, because nothing ran.
//!
//! LOCK ORDER: the cross-service reads here take the Secrets Manager store's lock (which
//! takes the IAM store's), each holding no other while it does, and hand back owned values —
//! so they are called with NO guard of this service's store held, the same contract
//! [`evaluate_image_pulls`] has.
//!
//! # Bounds
//!
//! At most [`MAX_FETCHED_SECRETS_PER_REVISION`] references of one revision are decided and
//! retrieved; past that, a reference is neither, so a revision referencing secrets without
//! end cannot make one launch do work without end.

use std::collections::BTreeMap;

use ::service_com_amazonaws_secretsmanager_secretsmanager::state::secret_retrieval;

use super::*;

/// One value a container definition has the agent fetch for it.
#[derive(Debug, Clone)]
pub struct ContainerSecret {
    /// `ContainerDefinition$name` of the container that takes it.
    pub container_name: String,
    /// `Secret$name`: "the name of the secret", which is the environment variable the value
    /// is exposed to the container as for a `secrets` entry, and the log-driver option it
    /// fills for a `secretOptions` one.
    pub name: String,
    /// `Secret$valueFrom`: "the secret to expose to the container".
    pub value_from: String,
    /// Whether this entry is a `secrets` entry, whose value the container's process sees as
    /// the environment variable [`Self::name`] — as against a `logConfiguration`
    /// `secretOptions` entry, which is "the secrets to pass to the log configuration" and so
    /// configures the log driver rather than the container's environment. Either way the
    /// agent retrieves it under the execution role, which is what the launch decision is
    /// about.
    pub environment_variable: bool,
}

/// The secrets every container of `definition` has the agent fetch, in the order the
/// revision lists them: each container's own `secrets`, then the `secretOptions` of its log
/// configuration.
///
/// The order is what decides WHICH refusal a launch referencing several unreadable secrets
/// stops with, so it is the revision's own order and nothing else.
pub fn definition_secrets(definition: &TaskDefinitionRecord) -> Vec<ContainerSecret> {
    let mut collected = Vec::new();
    for container in &definition.container_definitions {
        let container_name = container.name.clone().unwrap_or_default();
        for secret in container.secrets.iter().flatten() {
            collected.push(ContainerSecret {
                container_name: container_name.clone(),
                name: secret.name.clone(),
                value_from: secret.value_from.clone(),
                environment_variable: true,
            });
        }
        for secret in container
            .log_configuration
            .iter()
            .flat_map(|logging| logging.secret_options.iter().flatten())
        {
            collected.push(ContainerSecret {
                container_name: container_name.clone(),
                name: secret.name.clone(),
                value_from: secret.value_from.clone(),
                environment_variable: false,
            });
        }
    }
    if collected.len() > MAX_FETCHED_SECRETS_PER_REVISION {
        // The ceiling is this emulator's, so it is TRACED rather than passed over in
        // silence: a reference past it is neither decided nor retrieved, and the reader of
        // that task cannot otherwise tell that from a revision that referenced fewer.
        ::tracing::warn!(
            family = %definition.family,
            revision = definition.revision,
            referenced = collected.len(),
            limit = MAX_FETCHED_SECRETS_PER_REVISION,
            "this revision references more secrets than this emulator retrieves for one \
             revision, so the references past that are not retrieved"
        );
        collected.truncate(MAX_FETCHED_SECRETS_PER_REVISION);
    }
    collected
}

/// How many secret references ONE revision has this emulator retrieve. THIS NUMBER IS THE
/// EMULATOR'S, not an AWS quota: every reference is one in-process retrieval made for every
/// task of the revision, so without a ceiling a caller registering references without end
/// would have each launch do work without end. Far above what a realistic revision carries —
/// a revision holds at most [`MAX_CONTAINER_DEFINITIONS`] containers, so this is several
/// values per container.
pub const MAX_FETCHED_SECRETS_PER_REVISION: usize = 64;

/// The `secrets` of ONE container, as the environment variables its program is given them
/// in: the entries whose value the container's process sees
/// ([`ContainerSecret::environment_variable`]).
pub fn container_environment_secrets(
    definition: &TaskDefinitionRecord,
    container_name: &str,
) -> Vec<ContainerSecret> {
    definition_secrets(definition)
        .into_iter()
        .filter(|secret| secret.environment_variable && secret.container_name == container_name)
        .collect()
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// The secrets the revision `task` runs has the agent fetch for it
    /// ([`definition_secrets`]).
    ///
    /// Empty for a task whose revision this store no longer holds: a revision that is gone
    /// references nothing, so nothing is decided against it — the same reading the task role
    /// such a task runs under is resolved by.
    pub(super) fn launched_task_secrets(&self, task: &TaskRecord) -> Vec<ContainerSecret> {
        let Some((family, revision)) = parse_task_definition_reference(&task.task_definition_arn)
        else {
            return Vec::new();
        };
        revision
            .and_then(|revision| self.task_definitions.get(&(family, revision)))
            .map(definition_secrets)
            .unwrap_or_default()
    }
}

/// The error class Secrets Manager refuses an unauthorized read with, which is the AWS
/// common `AccessDeniedException` — the class the agent's own account of a failed retrieval
/// names.
const ACCESS_DENIED_CLASS: &str = "AccessDeniedException";

/// The error class Secrets Manager answers a read of a secret it holds nothing for with
/// (`ResourceNotFoundException`, "Secrets Manager can't find the resource that you asked
/// for", its own model).
const RESOURCE_NOT_FOUND_CLASS: &str = "ResourceNotFoundException";

/// How many attempts at the retrieval the stop's reason reports: the agent states how often
/// it retried the call it could not complete, and this emulator makes that call ONCE — the
/// answer it gets is the emulated service's own and does not change between attempts.
const RETRIEVAL_ATTEMPTS: usize = 1;

/// The `Task$stoppedReason` of a task whose containers could not be launched because an
/// execution resource could not be retrieved.
///
/// The error CLASS is the agent's `ResourceInitializationError`, which is what Amazon ECS
/// surfaces for a resource it could not fetch before starting a container, worded for the
/// Secrets Manager read (`asm`) the fetch was; `failure` is the answer that service gave,
/// which carries the authorization refusal or its own account of what it does not hold.
pub fn unable_to_retrieve_secret_message(failure: &str) -> String {
    format!(
        "ResourceInitializationError: unable to pull secrets or registry auth: execution \
         resource retrieval failed: unable to retrieve secret from asm: service call has been \
         retried {RETRIEVAL_ATTEMPTS} time(s): {failure}"
    )
}

/// The ARN of the SESSION Amazon ECS retrieves a task's execution resources in: it assumes
/// the execution role for the task, so the principal the retrieval is made by is an
/// assumed-role session of that role named after the task it is for
/// (`arn:<partition>:sts::<account>:assumed-role/<role name>/<task id>`).
///
/// `None` when `role_arn` is no IAM role ARN — there is no session to name for it.
pub fn execution_role_session_arn(role_arn: &str, task_id: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(role_arn)?;
    let role_name = parsed
        .resource
        .split('/')
        .next_back()
        .filter(|name| !name.is_empty())?;
    Some(format!(
        "arn:{}:sts::{}:assumed-role/{role_name}/{task_id}",
        parsed.partition, parsed.account
    ))
}

/// Why the execution resources of `launch` could not be retrieved — the `stoppedReason` of
/// the FIRST secret its execution role may not read, or that this account holds nothing for
/// — and `None` when every secret the revision references can be retrieved, or names a store
/// this module does not decide against.
///
/// Called with no guard of this service's store held: each verdict is the owning service's,
/// taken under that service's own lock ([`secret_retrieval::serve_secret_retrieval`]).
pub fn unretrievable_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    launch: &TaskLaunch,
) -> Option<String> {
    let task_id = launch
        .task_arn
        .rsplit('/')
        .next()
        .unwrap_or(launch.task_arn.as_str());
    let session_arn = launch
        .execution_role_arn
        .as_deref()
        .and_then(|role_arn| execution_role_session_arn(role_arn, task_id));
    let principal = launch
        .execution_role_arn
        .as_deref()
        .map(|role_arn| secret_retrieval::RetrievalPrincipal {
            role_arn,
            session_arn: session_arn.as_deref(),
        });
    launch.secrets.iter().find_map(|secret| {
        // A reference naming anything but a secret of Secrets Manager is not that service's
        // to answer, so nothing is decided for it here.
        let reference = SecretsManagerReference::parse(&secret.value_from)?;
        match secret_retrieval::serve_secret_retrieval(
            engine,
            &reference.region,
            &reference.secret_arn,
            principal,
        ) {
            Ok(()) => None,
            Err(secret_retrieval::RetrievalRefusal::NotAuthorized { message, .. }) => Some(
                unable_to_retrieve_secret_message(&format!("{ACCESS_DENIED_CLASS}: {message}")),
            ),
            Err(secret_retrieval::RetrievalRefusal::NotFound { message }) => {
                Some(unable_to_retrieve_secret_message(&format!(
                    "{RESOURCE_NOT_FOUND_CLASS}: {message}"
                )))
            }
        }
    })
}

/// What a `Secret$valueFrom` naming a Secrets Manager secret references.
///
/// The form is the one the `secrets` documentation gives for that store:
/// `arn:aws:secretsmanager:<region>:<account>:secret:<secret-name>:<json-key>:\
/// <version-stage>:<version-id>`, where each of the three selectors after the secret's own
/// ARN may be left empty to select the default — the whole value, the current version. A
/// secret's name carries no colon, so the fields after the ARN are exactly those selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretsManagerReference {
    /// The ARN of the secret itself: the reference with every selector taken off.
    pub secret_arn: String,
    /// The Region the secret lives in, read off its ARN — a secret is identified within one
    /// Region, so this is where the retrieval is addressed.
    pub region: String,
    /// The key of the JSON document held in the secret whose value is exposed, and `None`
    /// for a reference that exposes the whole value: "you can also specify a specific JSON
    /// key within the secret".
    pub json_key: Option<String>,
    /// The staging label the version is selected by, and `None` for the version Secrets
    /// Manager serves a read that names none.
    pub version_stage: Option<String>,
    /// The version id the version is selected by, and `None` for a reference naming none.
    pub version_id: Option<String>,
}

impl SecretsManagerReference {
    /// The reference `value_from` names when it addresses a Secrets Manager secret, and
    /// `None` for every other value — a Systems Manager parameter, or anything that is not
    /// an ARN of a secret at all.
    pub fn parse(value_from: &str) -> Option<Self> {
        let fields: Vec<&str> = value_from.split(':').collect();
        // `arn`, the partition, the vendor, the Region, the account, the `secret` resource
        // type and the secret's name — the six ARN fields with the type and the name as the
        // last of them.
        const NAME_FIELD: usize = 6;
        if fields.len() <= NAME_FIELD
            || fields[0] != "arn"
            || fields[2] != SECRETS_MANAGER_VENDOR
            || fields[5] != SECRET_RESOURCE_TYPE
            || fields[NAME_FIELD].is_empty()
        {
            return None;
        }
        let selector = |index: usize| {
            fields
                .get(index)
                .filter(|value| !value.is_empty())
                .map(|value| (*value).to_string())
        };
        Some(Self {
            secret_arn: fields[..=NAME_FIELD].join(":"),
            region: fields[3].to_string(),
            json_key: selector(NAME_FIELD + 1),
            version_stage: selector(NAME_FIELD + 2),
            version_id: selector(NAME_FIELD + 3),
        })
    }
}

/// The ARN vendor segment of a Secrets Manager secret, which is what tells a `valueFrom`
/// naming one from a `valueFrom` naming a Systems Manager parameter.
const SECRETS_MANAGER_VENDOR: &str = "secretsmanager";

/// The resource type a secret's ARN names
/// (`arn:aws:secretsmanager:<region>:<account>:secret:<name>-<six characters>`).
const SECRET_RESOURCE_TYPE: &str = "secret";

/// The environment a container's program runs with once the values the agent fetches for it
/// are in place: `environment` with every `secrets` entry's value under the name that entry
/// spells.
///
/// `None` when a secret the container takes could not be read at all — the container's
/// program is then not started, because the agent starts no container whose secrets it could
/// not resolve, and a container that never ran reports no exit code.
///
/// Each value is read through Secrets Manager's OWN `GetSecretValue` over the emulator's
/// in-process callback, with the version the reference selects, so what the container sees is
/// what a client reads back from that service. The value is never logged, and nothing of it
/// is kept in this service's store.
pub async fn fetched_environment(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    environment: &BTreeMap<String, String>,
    secrets: &[ContainerSecret],
) -> Option<BTreeMap<String, String>> {
    let mut resolved = environment.clone();
    for secret in secrets {
        // A reference this emulator resolves nothing for leaves the variable as the
        // container's own environment has it: the store it names is not this module's to
        // read, exactly as the launch decision does not decide against it.
        let Some(reference) = SecretsManagerReference::parse(&secret.value_from) else {
            continue;
        };
        let value = retrieved_value(callback, &reference).await?;
        resolved.insert(secret.name.clone(), value);
    }
    Some(resolved)
}

/// The value `reference` exposes: the secret's string value, or the value of the JSON key
/// the reference selects within it.
///
/// `None` when the retrieval was refused, when the secret holds no string value, or when the
/// key the reference selects is not in the document — each of them a resource the agent could
/// not retrieve, so the container it was for starts nothing.
async fn retrieved_value(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    reference: &SecretsManagerReference,
) -> Option<String> {
    let mut request = secrets_manager_client(callback, &reference.region)
        .get_secret_value()
        .secret_id(&reference.secret_arn);
    if let Some(stage) = &reference.version_stage {
        request = request.version_stage(stage);
    }
    if let Some(version_id) = &reference.version_id {
        request = request.version_id(version_id);
    }
    let answer = match request.send().await {
        Ok(answer) => answer,
        Err(error) => {
            // Visible to an operator: this is why the container was not started. The
            // reference is named; nothing of any value is.
            ::tracing::warn!(
                secret_arn = %reference.secret_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a secret a container takes could not be retrieved, so its program is not run"
            );
            return None;
        }
    };
    let held = answer.secret_string()?;
    let Some(json_key) = &reference.json_key else {
        return Some(held.to_string());
    };
    // "You can also specify a specific JSON key within the secret": the value is read as the
    // key/value document a secret of that shape holds, and the key's own value is what the
    // container is given — a document that states no such key states no value for it.
    let document: ::serde_json::Value = ::serde_json::from_str(held).ok()?;
    let selected = document.get(json_key)?;
    Some(match selected {
        // A string field is exposed as its characters; every other JSON value as the text
        // the document writes it with.
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

/// A Secrets Manager client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no socket is opened and no name is resolved. The configuration is
/// the one every callback client in this workspace uses (see
/// [`super::container_logs`]): nothing to retry, time out or watch for a stalled stream,
/// because the transport is in process, and the credentials the callback hands over are the
/// emulator's own — WHICH role may read the secret is decided ahead of the retrieval, against
/// that role's own documents ([`unretrievable_secret`]).
///
/// `region` is ADDRESSING for that transport rather than a host anything connects to: it is
/// the Region the secret's own ARN names, which is the Region that holds it.
fn secrets_manager_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_secretsmanager::Client {
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `valueFrom` naming a Secrets Manager secret parses into the secret's own ARN
    /// and the selectors written after it; every other value names no secret of that store.
    #[test]
    fn a_value_from_parses_into_the_secret_and_its_selectors() {
        let arn = "arn:aws:secretsmanager:us-west-2:000000000000:secret:injected-AbCdEf";
        // (case, valueFrom, the reference it names)
        let rows: &[(&str, String, Option<SecretsManagerReference>)] = &[
            (
                "the secret's own ARN",
                arn.to_string(),
                Some(SecretsManagerReference {
                    secret_arn: arn.to_string(),
                    region: "us-west-2".to_string(),
                    json_key: None,
                    version_stage: None,
                    version_id: None,
                }),
            ),
            (
                "a JSON key within it",
                format!("{arn}:password"),
                Some(SecretsManagerReference {
                    secret_arn: arn.to_string(),
                    region: "us-west-2".to_string(),
                    json_key: Some("password".to_string()),
                    version_stage: None,
                    version_id: None,
                }),
            ),
            (
                "a staging label with no JSON key",
                format!("{arn}::AWSPREVIOUS"),
                Some(SecretsManagerReference {
                    secret_arn: arn.to_string(),
                    region: "us-west-2".to_string(),
                    json_key: None,
                    version_stage: Some("AWSPREVIOUS".to_string()),
                    version_id: None,
                }),
            ),
            (
                "every selector at once",
                format!("{arn}:password:AWSCURRENT:v1"),
                Some(SecretsManagerReference {
                    secret_arn: arn.to_string(),
                    region: "us-west-2".to_string(),
                    json_key: Some("password".to_string()),
                    version_stage: Some("AWSCURRENT".to_string()),
                    version_id: Some("v1".to_string()),
                }),
            ),
            (
                "a Systems Manager parameter",
                "arn:aws:ssm:us-west-2:000000000000:parameter/injected".to_string(),
                None,
            ),
            ("a bare parameter name", "/injected".to_string(), None),
            (
                "an ARN of another resource type of the same service",
                "arn:aws:secretsmanager:us-west-2:000000000000:other:injected".to_string(),
                None,
            ),
            (
                "an ARN naming no secret",
                "arn:aws:secretsmanager:us-west-2:000000000000:secret:".to_string(),
                None,
            ),
        ];
        for (case, value_from, expected) in rows {
            let actual = SecretsManagerReference::parse(value_from);
            assert_eq!(
                actual.as_ref(),
                expected.as_ref(),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the principal a retrieval is made by is an assumed-role session of the
    /// execution role, named after the task it is made for.
    #[test]
    fn the_retrieval_principal_is_a_session_of_the_execution_role() {
        // (case, the role ARN, the session ARN it names for task `t-1`)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a role of this account",
                "arn:aws:iam::000000000000:role/exec",
                Some("arn:aws:sts::000000000000:assumed-role/exec/t-1"),
            ),
            (
                "a role under a path",
                "arn:aws:iam::000000000000:role/team/exec",
                Some("arn:aws:sts::000000000000:assumed-role/exec/t-1"),
            ),
            (
                "a role of another partition",
                "arn:aws-us-gov:iam::000000000000:role/exec",
                Some("arn:aws-us-gov:sts::000000000000:assumed-role/exec/t-1"),
            ),
            ("a bare role name", "exec", None),
        ];
        for (case, role_arn, expected) in rows {
            let actual = execution_role_session_arn(role_arn, "t-1");
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
