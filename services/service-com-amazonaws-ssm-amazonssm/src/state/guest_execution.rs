// SPDX-License-Identifier: CC0-1.0

//! Running a command document's own script IN THE GUEST of the node it was sent to.
//!
//! # What a Run Command invocation is
//!
//! "Run Command … lets you remotely and securely manage … run commands on your managed
//! nodes" — a command's invocation is the document's script PERFORMED on the node, and what
//! the client reads back is what that script wrote and the exit code it ended on
//! (`GetCommandInvocation`'s `StandardOutputContent` and `ResponseCode`). A send that only
//! recorded the request would therefore implement nothing: the invocation would never
//! finish, and a client polling for a terminal status would poll for ever.
//!
//! # Where it runs
//!
//! The agent that runs it is INSIDE the guest, and the guest belongs to the service that
//! owns instances: its storage, its host name and the identity the kernel published to it
//! are facts of the EC2 instance. So this module asks that service to perform the script in
//! that machine, through its own seam
//! (`ComAmazonawsEc2Amazonec2State::run_in_instance_guest`, which states what a run is and
//! what it is not) — the same bounded, in-process shell session the instance's own boot
//! script runs in. There is no process, no socket and no host path in it, a command the
//! session provides no program for answers the status a shell answers for a utility it could
//! not find, and nothing is reported for a node whose guest is not up.
//!
//! # Which script
//!
//! The script is the one the DOCUMENT declares for the node's platform: the step whose
//! plugin runs on it ([`super::document_platforms::step_runs_on`]), with the document's
//! `commands` input resolved from the parameters the send supplied and the defaults the
//! document declares. Those resolved lines are ONE script, which is what the agent performs
//! with the guest's shell (`ComAmazonawsEc2Amazonec2State::run_in_instance_guest_with`), so a
//! construct spanning them is the script's own and what the invocation reports is that
//! script's exit status and its two streams. A document that declares no step this node runs
//! — a Windows document sent to a Linux node — is not something the node's agent performs,
//! and the send's own `UnsupportedPlatformType` refusal is what a client is told about that.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2_store;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::aws_api::CallerIdentity;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::vpc_network;
use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::AwsAccess;

use crate::ops::types::DocumentParameter;

use super::document_references::whole_reference;
use super::{load_single_document, mapping_get, scalar_string};

/// The input of a command document's step that carries the script it runs: "commands" is
/// the input `aws:runShellScript` and `aws:runPowerShellScript` both take the script in
/// (the Systems Manager command document plugin reference).
pub const COMMANDS_INPUT: &str = "commands";

/// The same input under its schema 1.2 spelling: a schema 1.2 document declares its steps
/// as `runtimeConfig` plugin entries, and the plugin property that carries the script
/// there is `runCommand` (the "SSM document schema 1.2" plugin reference) — so both
/// spellings name the script of the one step a node's agent performs.
pub const RUN_COMMAND_PROPERTY: &str = "runCommand";

/// Where a schema 1.2 plugin entry declares the properties one invocation of it runs
/// with: "properties" holds either one property mapping or a list of them.
const PLUGIN_PROPERTIES: &str = "properties";

/// The step of `content` a node of `platform_type` runs, with its script resolved: the
/// step's name and the lines the guest performs.
#[derive(Debug, Clone)]
pub struct ShellStep {
    /// The step's own `name`, which is what `GetCommandInvocation`'s `PluginName` names.
    pub name: String,
    /// The lines of the script, in the document's own order.
    pub commands: Vec<String>,
}

/// The step of the document `content` that a node of `platform_type` runs, or `None` when
/// the content declares no step with a script that node runs.
///
/// `supplied` are the parameter values the send carried and `declared` the document's own
/// parameters, whose `DefaultValue` fills in for one the send left out — "If you don't
/// specify a value, the default value is used" is what a document's default is for.
///
/// Both spellings of a step are read, the way every other reader of a document's steps
/// reads them ([`super::documents::declared_step_names`],
/// [`super::document_platforms::declared_platform_types`]): a schema 2.x document declares
/// its steps as `mainSteps`, each with its own `action` and `inputs`, and a schema 1.2
/// document declares them as the `runtimeConfig` plugin entries, whose keys are the plugin
/// names and whose `properties` carry the script.
pub fn shell_step(
    content: &str,
    platform_type: &str,
    supplied: &BTreeMap<String, Vec<String>>,
    declared: &[DocumentParameter],
) -> Option<ShellStep> {
    let document = load_single_document(content).ok()?;
    let map = document.as_hash()?;
    let values = parameter_values(supplied, declared);
    if let Some(steps) = mapping_get(map, "mainSteps").and_then(::yaml_rust2::Yaml::as_vec) {
        return main_steps_script(steps, platform_type, &values);
    }
    runtime_config_script(map, platform_type, &values)
}

/// The step of a schema 2.x document's `mainSteps` that a node of `platform_type` runs,
/// with its script resolved.
fn main_steps_script(
    steps: &[::yaml_rust2::Yaml],
    platform_type: &str,
    values: &BTreeMap<String, Vec<String>>,
) -> Option<ShellStep> {
    for step in steps {
        let step = step.as_hash()?;
        if !super::document_platforms::step_runs_on(step, platform_type) {
            continue;
        }
        let inputs = mapping_get(step, "inputs").and_then(::yaml_rust2::Yaml::as_hash);
        let Some(script) = inputs.and_then(|inputs| mapping_get(inputs, COMMANDS_INPUT)) else {
            // A step of this document that takes no script is one with nothing for the
            // guest's shell to perform; another step of the same document may still have
            // one.
            continue;
        };
        let commands = resolved_commands(script, values);
        if commands.is_empty() {
            continue;
        }
        return Some(ShellStep {
            name: mapping_get(step, "name")
                .and_then(scalar_string)
                .unwrap_or_default(),
            commands,
        });
    }
    None
}

/// The schema 1.2 plugin entry of `map` that a node of `platform_type` runs, with its
/// script resolved.
///
/// A plugin entry carries no precondition, so the PLUGIN alone decides where it runs
/// ([`super::document_platforms::plugin_platform_types`]), and the step's name — what
/// `GetCommandInvocation`'s `PluginName` names for such a document — is the plugin's own
/// name, which is the key it is declared under.
fn runtime_config_script(
    map: &::yaml_rust2::yaml::Hash,
    platform_type: &str,
    values: &BTreeMap<String, Vec<String>>,
) -> Option<ShellStep> {
    let plugins = mapping_get(map, "runtimeConfig").and_then(::yaml_rust2::Yaml::as_hash)?;
    for (name, plugin) in plugins {
        let Some(name) = name.as_str() else {
            continue;
        };
        let runs_here = super::document_platforms::plugin_platform_types(name)
            .is_none_or(|platforms| platforms.contains(&platform_type));
        if !runs_here {
            continue;
        }
        let Some(plugin) = plugin.as_hash() else {
            continue;
        };
        for properties in plugin_properties(plugin) {
            let Some(script) = mapping_get(properties, RUN_COMMAND_PROPERTY)
                .or_else(|| mapping_get(properties, COMMANDS_INPUT))
            else {
                continue;
            };
            let commands = resolved_commands(script, values);
            if commands.is_empty() {
                continue;
            }
            return Some(ShellStep {
                name: name.to_string(),
                commands,
            });
        }
    }
    None
}

/// The property mappings one schema 1.2 plugin entry declares: `properties` is written
/// either as one mapping or as a list of them, and a plugin that declares none runs
/// nothing.
fn plugin_properties(plugin: &::yaml_rust2::yaml::Hash) -> Vec<&::yaml_rust2::yaml::Hash> {
    let Some(properties) = mapping_get(plugin, PLUGIN_PROPERTIES) else {
        return Vec::new();
    };
    match properties {
        ::yaml_rust2::Yaml::Array(entries) => {
            entries.iter().filter_map(::yaml_rust2::Yaml::as_hash).collect()
        }
        other => other.as_hash().into_iter().collect(),
    }
}

/// The lines of ONE step's script, resolved against `values` and held to the one budget
/// the whole script is built against ([`MAX_SCRIPT_BYTES`], spent across every level the
/// content nests).
fn resolved_commands(
    script: &::yaml_rust2::Yaml,
    values: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut budget = MAX_SCRIPT_BYTES;
    resolved_script(script, values, 0, &mut budget)
}

/// The value of every parameter the document has one for: the document's declared default
/// first, then whatever the send supplied for it.
fn parameter_values(
    supplied: &BTreeMap<String, Vec<String>>,
    declared: &[DocumentParameter],
) -> BTreeMap<String, Vec<String>> {
    let mut values: BTreeMap<String, Vec<String>> = declared
        .iter()
        .filter_map(|parameter| {
            let name = parameter.name.clone()?;
            let default = parameter.default_value.clone()?;
            Some((name, vec![default]))
        })
        .collect();
    for (name, value) in supplied {
        values.insert(name.clone(), value.clone());
    }
    values
}

/// How deeply a step's `commands` input is walked: the input is a list of lines, so one
/// level of nesting is all a document needs, and this leaves room for a document that nests
/// a list inside one.
///
/// The content is a caller's text and the walk recurses once per level, so the depth it can
/// ask for is bounded here: a stack this emulator cannot unwind from is not a refusal, it is
/// the process gone.
const MAX_SCRIPT_DEPTH: usize = 8;

/// How many lines of script one step yields, however many the document and its parameters
/// describe between them.
const MAX_SCRIPT_LINES: usize = 256;

/// How many bytes ONE line of script comes to once its parameter references are resolved.
const MAX_SCRIPT_LINE_BYTES: usize = 8 * 1024;

/// How many bytes the WHOLE resolved script of one step comes to, over every line of it and
/// every level the content nests.
///
/// Both halves of what reaches this walk are a caller's — the document's content and the
/// parameter values the send carried — and one line of the content may name a parameter whose
/// value is a list, so the lines and the bytes multiply. The line count and the per-line
/// ceiling bound each factor alone; this bounds their PRODUCT, so the script is built against
/// one budget whichever shape the document takes. It is the emulator's own number, matched to
/// what the guest will take at all (`…ec2…::state::guest_commands`'s `MAX_GUEST_SCRIPT_BYTES`):
/// a script past it is refused where it is performed anyway, so nothing is gained by building
/// it first.
const MAX_SCRIPT_BYTES: usize = 256 * 1024;

/// The lines of the script a step's `commands` input holds, with the document's parameter
/// references resolved.
///
/// The input is written either as ONE reference to a `StringList` parameter
/// (`"commands": "{{ commands }}"`, which is how the published command documents take a
/// caller's script) or as a list of lines, each of which may carry references of its own.
/// A reference that IS the whole value resolves to the list it names, line for line; one
/// inside a line resolves to its value in that line.
///
/// Bounded in every direction a caller chooses: how deep the content nests
/// ([`MAX_SCRIPT_DEPTH`]), how many lines it comes to ([`MAX_SCRIPT_LINES`]), how long one
/// resolved line is ([`MAX_SCRIPT_LINE_BYTES`]) and how many bytes the whole script comes to
/// ([`MAX_SCRIPT_BYTES`], spent across every level of the walk). A line that would run past
/// the budget is not built, and the walk stops there: the guest is handed the lines that fit
/// rather than a script this process grew without a ceiling.
fn resolved_script(
    script: &::yaml_rust2::Yaml,
    values: &BTreeMap<String, Vec<String>>,
    depth: usize,
    budget: &mut usize,
) -> Vec<String> {
    if depth >= MAX_SCRIPT_DEPTH {
        return Vec::new();
    }
    match script {
        ::yaml_rust2::Yaml::Array(lines) => {
            let mut resolved: Vec<String> = Vec::new();
            for line in lines {
                if resolved.len() >= MAX_SCRIPT_LINES || *budget == 0 {
                    break;
                }
                resolved.extend(resolved_script(line, values, depth + 1, budget));
            }
            resolved.truncate(MAX_SCRIPT_LINES);
            resolved
        }
        other => {
            let Some(text) = scalar_string(other) else {
                return Vec::new();
            };
            let named = match whole_reference(&text).and_then(|name| values.get(name)) {
                // A reference that IS the whole value resolves to the list it names, line for
                // line — and each of those lines is spent from the one budget, so a value the
                // send carried cannot be cloned wholesale past it.
                Some(named) => named.iter().take(MAX_SCRIPT_LINES).cloned().collect(),
                None => vec![resolved_line(&text, values)],
            };
            let mut lines: Vec<String> = Vec::new();
            for line in named {
                if line.len() > MAX_SCRIPT_LINE_BYTES || line.len() > *budget {
                    break;
                }
                *budget -= line.len();
                lines.push(line);
            }
            lines
        }
    }
}

/// One line of script with its `{{ name }}` references resolved — a list parameter joined
/// by commas, which is how a reference to one inside a line reads.
///
/// The scan and the substitution are the shared ones every document of this service reads
/// its references through ([`super::document_references`]), measured before they are built
/// and held to [`MAX_SCRIPT_LINE_BYTES`]. A reference to a parameter the send carried no
/// value for is left as the document wrote it: the document asked for something the request
/// did not carry, which the send's own parameter validation is what refuses. A line whose
/// resolution would run past the ceiling resolves to NOTHING, so the guest is handed no such
/// line rather than a truncated one.
fn resolved_line(text: &str, values: &BTreeMap<String, Vec<String>>) -> String {
    let value_of = |name: &str| {
        Some(
            values
                .get(name)
                .map(|value| value.join(","))
                // The reference as written, which the scan does not look at again.
                .unwrap_or_else(|| format!("{{{{ {name} }}}}")),
        )
    };
    let mut budget = MAX_SCRIPT_LINE_BYTES;
    super::document_references::resolved_text(text, &value_of, &mut budget).unwrap_or_default()
}

/// Perform `commands` in the guest of the EC2 instance `instance_id` names, answering what
/// the guest wrote and the status it ended on — or `None` when this engine holds no EC2
/// store, or that store holds no running, booted guest for the id.
///
/// The boot the guest was already owed is materialised first: an instance that has just
/// started has its operating system coming up at its own instant, and a script runs in the
/// guest that boot brought up.
///
/// The session is given the resolver clients of the guest besides
/// ([`super::guest_resolver`]): a script that resolves a name is answered by the DNS of the
/// VPC the node is in, which is read ONCE here — before the store that owns the guest is
/// taken — because answering a query reads three stores and running the script holds one. The
/// the guest's package-query client travels the same way ([`super::guest_packages`]): which
/// build of the agent its package database holds is a fact of the NODE, which this service holds
/// and the service that owns the guest cannot read. So does the guest's PYTHON INTERPRETER
/// ([`super::guest_python`]), which runs the inline program a script that reports a computed
/// line ends in: the one contained engine this workspace embeds for caller-supplied Python is
/// reachable from this crate and not from the one that owns the guest — and so does the AWS CLI
/// the machine's image ships ([`super::guest_aws_cli`]), whose object calls are made on the
/// machine's own terms. `programs` is what the
/// programs of ONE REQUEST may spend between them
/// ([`super::guest_python::ProgramAllowance`]) — made once per request and handed to every
/// delivery of it, which is what bounds the product of the commands a script carries and the
/// nodes a send selects.
///
/// The machine's own IDENTITY travels the same way: which role the instance's instance
/// profile carries is a fact IAM holds, so that service's own records answer it
/// ([`ec2_store::instance_profile_role_name`], read before the guest's store is taken), and
/// the guest's metadata service publishes it to a program that asks — the read an agent makes
/// to learn the role it is running as. The CREDENTIALS of that role travel with it, minted by
/// the token service for the instance ([`ec2_store::instance_role_credentials`]) out of the
/// emulator's own credential store, which only a request's own context carries: a script inside
/// the guest signs its AWS calls with them, so a call it makes is a call made by the role the
/// instance runs as. The terms those calls are made on travel beside them
/// ([`MachineAccess`]).
pub fn perform_in_guest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
    commands: &[String],
    now: SystemTime,
    programs: &::std::sync::Arc<super::guest_python::ProgramAllowance>,
) -> Option<ec2_store::GuestRun> {
    let delivery = delivery_in_guest(engine, ctx, region, instance_id, now, programs);
    perform_delivery(engine, instance_id, commands, delivery)
}

/// Everything ONE DELIVERY needs to perform a script in a guest, OWNED: the programs the
/// caller installs in that guest beside the machine's own, and the part of the machine's
/// identity another service holds.
///
/// Resolved with the REQUEST's own context ([`delivery_in_guest`], which reads this service's
/// store, the account's DNS, the VPC's file systems and the emulator's credential store) so
/// that the RUN itself ([`perform_delivery`]) borrows nothing of the request and can be
/// performed on a thread of its own — which is what lets a caller keep the asynchronous runtime
/// responsive while a script runs.
pub struct GuestDelivery {
    /// The programs the caller installs in the guest beside the machine's own.
    supplied: Option<Box<dyn ::service_common::posix_shell_session::EnvironmentPrograms>>,
    /// The part of the machine's own identity another service owns.
    identity: ec2_store::GuestIdentity,
}

impl ::std::fmt::Debug for GuestDelivery {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("GuestDelivery")
            .field("supplied", &self.supplied)
            .field("identity", &self.identity)
            .finish()
    }
}

/// The delivery for the node `instance_id` names: the programs this service installs in that
/// guest ([`GuestDelivery`]) and the machine's own identity, read with the request's context.
///
/// Every read here happens BEFORE the store that owns the guest is taken, and none of them is
/// held across the run.
#[must_use]
pub fn delivery_in_guest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
    now: SystemTime,
    programs: &::std::sync::Arc<super::guest_python::ProgramAllowance>,
) -> GuestDelivery {
    let credentials = ctx.credential_store();
    let resolver = ::service_com_amazonaws_route53resolver_route53resolver::state::vpc_dns::view_from_instance(
        engine, instance_id,
    )
    .map(super::guest_resolver::resolver_programs);
    // ... and the package-query client ([`super::guest_packages`]), answered out of the AGENT
    // BUILD this service holds for the node — read here, before the store that owns the guest
    // is taken, because it is a read of this service's own node store.
    let packages = agent_package_programs(engine, region, instance_id);
    // ... and the NFS client of the node's own VPC, which is how a script MOUNTS a file system
    // of Amazon EFS: which address serves NFS, which file system is behind it and the files
    // that file system holds are facts of that service, read through its own data-plane
    // accessor before the store that owns the guest is taken.
    let node_vpc_id = node_vpc_id(engine, instance_id);
    let nfs = ::service_com_amazonaws_efs_magnolioapiservice_v20150201::state::nfs_client_programs(
        engine,
        now,
        node_vpc_id.as_deref(),
    );
    // ... and the file-system client of S3 Files, read the same way and for the same reason: a
    // script that mounts one of ITS file systems names the file system itself or the address
    // its mount target reports, and both are that service's facts. It is asked BEFORE the NFS
    // client above, because a mount either data plane performs nothing of falls through to the
    // other, so each answers for its own file systems only.
    let s3_files = ::service_com_amazonaws_s3files_s3files::state::mount_client_programs(
        engine,
        now,
        node_vpc_id.as_deref(),
    );
    // ... and the PYTHON INTERPRETER the inline programs of such a script are written in
    // ([`super::guest_python`]): the one contained engine this workspace embeds for running
    // caller-supplied Python, which the service that owns the guest cannot reach. It spends
    // from `programs`, the allowance the REQUEST's own programs share, so what one request can
    // make this process run is bounded across every command of every script it delivers.
    //
    // ... and the AWS CLI the machine's image ships ([`super::guest_aws_cli`]), which a script
    // that moves data invokes. BOTH reach the rest of this emulator on the MACHINE's own terms
    // ([`machine_access`]): signed as the role the instance profile carries, and held to what
    // the machine's network carries — read here, off two other services' stores and this
    // request's own context, for the reason everything else here is.
    let machine = machine_access(engine, ctx, instance_id, now);
    let interpreter = Some(super::guest_python::interpreter_programs(
        programs,
        machine
            .as_ref()
            .and_then(|machine| machine.sdk_access(ctx, region)),
    ));
    let command_line = machine.as_ref().and_then(|machine| {
        super::guest_aws_cli::aws_cli_programs(
            ctx.callback().clone(),
            region,
            Some(machine.signing_as()),
            machine.egress.clone(),
        )
    });
    GuestDelivery {
        supplied: [resolver, packages, s3_files, nfs, interpreter, command_line]
            .into_iter()
            .flatten()
            .reduce(::service_common::posix_shell_session::layered),
        identity: ec2_store::GuestIdentity {
            iam_role_name: ec2_store::instance_profile_role_name(engine, instance_id),
            iam_role_credentials: ec2_store::instance_role_credentials(
                engine,
                credentials,
                instance_id,
                now,
            ),
        },
    }
}

/// What the MACHINE brings to the programs a caller installs in its guest: WHO their calls are
/// made by, and WHAT its own network carries.
///
/// Two facts of the machine, each read through the store that owns it:
///
/// * WHO the call is made by. "Applications on the instance retrieve the security credentials
///   provided by the role from the instance metadata" (Amazon EC2 User Guide, "Use an IAM role
///   to grant permissions to applications running on Amazon EC2 instances"), so a call made from
///   inside the guest is a call made by the role the instance profile carries: it is signed as
///   the SESSION the service that owns instances mints for the machine
///   ([`ec2_store::instance_role_credentials`]), under the identity that mint names — the
///   session of that role, named for the instance
///   ([`::service_common::iam_role::assumed_role_session_arn`]) — so the service serving the
///   call judges it against the policies in force on the role, and a refusal says which
///   instance was refused. An instance that runs as no profile has no credentials in its guest
///   at all, and nothing here signs for it.
/// * WHAT its own NETWORK carries. A call out of the machine travels through the VPC endpoints
///   of its VPC, whose policies are that service's records
///   ([`ec2_store::instance_endpoint_policies`]), and a call such a policy does not allow is
///   refused by the called service however the role's own policies read — which is what an
///   endpoint policy is for ([`::service_common::vpc_endpoint_calls`], where that decision
///   lives). A machine whose VPC holds no endpoint has no such control in force on it.
/// * WHICH NETWORK that is. The machine stands inside a VPC, on the elastic network interfaces
///   its launch attached, so a name a program of its guest looks up is asked of the DNS of THAT
///   VPC and an address it reaches is an address of it — the one reading of a VPC's own network
///   this workspace holds, built out of those interfaces ([`vpc_network::attachment_of`]). An
///   instance whose interfaces place it in no VPC this emulator holds has no such network, and
///   its programs resolve only what needs no zone of an account at all.
///
/// Read ONCE per delivery, before the store that owns the guest is taken, and shared by every
/// program installed in that guest — so the interpreter's own SDK call and the command line's
/// are made by the same identity on the same terms.
struct MachineAccess {
    /// The session the machine's calls are signed as. Its debug form carries no field.
    credentials: ec2_store::GuestRoleCredentials,
    /// What its network carries, and `None` where no such control is in force.
    egress: Option<::std::sync::Arc<dyn ::service_common::vpc_endpoint_calls::CallEgress>>,
    /// The VPC the machine stands IN, as its own interfaces place it there — and `None` for an
    /// instance this emulator can place in no VPC.
    attachment: Option<vpc_network::VpcAttachment>,
}

impl ::std::fmt::Debug for MachineAccess {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("MachineAccess")
            .field("egress", &self.egress)
            .field("attachment", &self.attachment)
            .finish_non_exhaustive()
    }
}

impl MachineAccess {
    /// The three values a call of this machine is signed with, as the programs installed in its
    /// guest present them. Held by host code: they reach the request's own signature and
    /// nothing else.
    fn signing_as(&self) -> (String, String, String) {
        (
            self.credentials.access_key_id.clone(),
            self.credentials.secret_access_key.clone(),
            self.credentials.session_token.clone(),
        )
    }

    /// The SEAM an AWS SDK call made inside this guest is dispatched on, and the NETWORK the
    /// programs installed in that guest reach through it: the in-process callback, signed as the
    /// machine's own session, held to what its network carries, and standing inside the machine's
    /// own VPC — so a name one of those programs resolves is answered by that VPC's DNS, and an
    /// address it opens a connection to is an address of that VPC.
    ///
    /// `None` where there is no seam at all — no async runtime to dispatch a call on — in which
    /// case a program simply has no SDK and says so.
    fn sdk_access(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
    ) -> Option<AwsAccess> {
        let (access_key_id, secret_access_key, session_token) = self.signing_as();
        Some(
            AwsAccess::new(ctx.callback().clone(), region)?
                .signing_as(Some(CallerIdentity {
                    access_key_id,
                    secret_access_key,
                    session_token,
                }))
                .addressed_at(ctx.endpoint())
                .leaving_through(self.egress.clone())
                .attached_in_vpc(self.attachment.clone()),
        )
    }
}

/// What the machine `instance_id` names brings to its guest's programs ([`MachineAccess`]) —
/// `None` for an instance that runs as no instance profile, or one this emulator could mint no
/// session for, whose guest then holds no credential at all.
fn machine_access<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    instance_id: &str,
    now: SystemTime,
) -> Option<MachineAccess> {
    let role = ec2_store::instance_profile_role(engine, instance_id)?;
    let credentials =
        ec2_store::instance_role_credentials(engine, ctx.credential_store(), instance_id, now)?;
    // The identity the mint names the session by: the role's own session, named for the
    // instance, which is what a refusal of one of its calls is about.
    let principal = ::service_common::iam_role::assumed_role_session_arn(&role.arn, instance_id);
    let egress = ::service_common::vpc_endpoint_calls::VpcEndpointEgress::of(
        ec2_store::instance_endpoint_policies(engine, instance_id),
        &principal,
        ::engine::emulator_account_id(),
    );
    Some(MachineAccess {
        credentials,
        egress: egress.map(|egress| {
            ::std::sync::Arc::new(egress)
                as ::std::sync::Arc<dyn ::service_common::vpc_endpoint_calls::CallEgress>
        }),
        attachment: node_attachment(engine, ctx, instance_id),
    })
}

/// The VPC the node `instance_id` names stands in, as the interfaces its launch attached place
/// it there — and `None` for an instance whose interfaces this emulator can place in no VPC.
///
/// The attachment is the one reading of a VPC's own network this workspace holds
/// ([`vpc_network::attachment_of`], which reads the account's EC2 inventory for the addresses,
/// the ranges, the security groups and whether that VPC's DNS answers at all): the interfaces
/// are the machine's, read off the store that owns them, so what a program of this guest
/// resolves and reaches is the same network a query the guest's own resolver client sends is
/// answered by.
///
/// LOCK ORDER: this takes the EC2 store's lock, gives it up, and is called BEFORE that store is
/// taken for the run itself.
fn node_attachment<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    instance_id: &str,
) -> Option<vpc_network::VpcAttachment> {
    let interface_ids: Vec<String> = {
        let ec2 = ec2_store::service_state(engine)?;
        let store = ec2.lock();
        store
            .instance_network_interfaces(instance_id)
            .into_iter()
            .filter_map(|interface| interface.network_interface_id)
            .collect()
    };
    vpc_network::attachment_of(engine, &super::request_region(ctx.parts()), &interface_ids)
}

/// Perform `commands` in the guest of the instance `instance_id` names, with `delivery` as
/// what the caller installs in it — the run itself, which borrows nothing of the request.
///
/// SYNCHRONOUS and CPU-bound: the script is performed by the shell session of the service that
/// owns the guest, and a program of the guest may wait a bounded span for an interpreter of
/// this process. A caller serving a request therefore performs it on a BLOCKING thread rather
/// than on a worker of the asynchronous runtime (`crate::ops::send_command`).
///
/// NO guard of the store that owns the guest is held while the script runs: the programs
/// installed in that guest CALL this emulator — the interpreter's own SDK and the machine's
/// command line both reach it over the in-process callback, on this very thread — and such a
/// call is served by a handler that may need that store. So the run is performed in the three
/// steps that service states for exactly this reason
/// ([`ec2_store::ComAmazonawsEc2Amazonec2State::prepare_instance_guest_run`]): the session is
/// built under the guard, the script runs with the guard given up, and what it left is kept
/// under the guard again.
#[must_use]
pub fn perform_delivery<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_id: &str,
    commands: &[String],
    delivery: GuestDelivery,
) -> Option<ec2_store::GuestRun> {
    let ec2 = ec2_store::service_state(engine)?;
    let prepared = {
        let mut store = ec2.lock();
        // The moves that service already owed the instance — its launch reaching `running` and
        // its guest's first boot — were due at their own instants whether or not anybody called,
        // and a script runs in the guest that boot brought up. Materialising them is EC2's own
        // write, through its own one-stop materialiser.
        store.materialize_due_transitions();
        store.prepare_instance_guest_run(instance_id, delivery.supplied, &delivery.identity)
    }?;
    // The run itself, with no guard of that store held — so a call a program of the guest makes
    // is served rather than waiting for a guard its own script holds.
    let performed = prepared.performed(commands);
    let run = performed.map(|performed| ec2.lock().commit_instance_guest_run(performed));
    // What the script left the guest's own PUBLISHERS owing is published with no store guard
    // held: a CloudWatch agent the script configured and started has its first collections and
    // the StatsD values its listener received to publish, and those datapoints are the store
    // that owns them (CloudWatch's) to take. It is the service that owns the guest that
    // publishes them, through its own seam, so this delivery reaching a guest is what makes
    // them due rather than a second publisher of the same series.
    ec2_store::publish_instance_metrics(engine);
    run
}

/// The VPC the node `instance_id` names is IN, as the service that owns instances reports it
/// — and `None` for an instance that service holds none, or that it can place in no VPC.
///
/// Which VPC a machine stands in decides which addresses of the account it can reach at all,
/// so a program of its guest that reaches a resource by a private address is offered the ones
/// of that VPC and no other. The fact is EC2's own — an instance's interface is in a subnet,
/// and the subnet is in the VPC — so it is read off that store rather than kept a second time
/// here.
///
/// LOCK ORDER: this takes the EC2 store's lock and holds no other while it does, and is called
/// BEFORE that store is taken for the run itself.
fn node_vpc_id<Eng: Send + Sync + 'static>(engine: &Eng, instance_id: &str) -> Option<String> {
    let ec2 = ec2_store::service_state(engine)?;
    let store = ec2.lock();
    let instance = store.instances.get(instance_id)?;
    store.instance_vpc_id(instance)
}

/// The package-query client for the guest of the node `instance_id` names in `region`, or
/// `None` when this endpoint holds no such node — an instance that is no managed node has no
/// agent this service reported a build for, and nothing here invents one.
///
/// The build and the machine's architecture are the NODE's own
/// (`InstanceInformation$AgentVersion`, `InstanceProperty$Architecture`), so what the guest's
/// package database answers and what this service reports for the node are one fact. Every
/// OTHER package that database holds is the owning service's record of the machine, answered by
/// that machine's own program, which the session asks first.
fn agent_package_programs<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    instance_id: &str,
) -> Option<Box<dyn ::service_common::posix_shell_session::EnvironmentPrograms>> {
    let state = super::service_state(engine)?;
    let store = state.lock();
    let node = store.managed_node(region, instance_id)?;
    let version = node.properties.agent_version.clone()?;
    let architecture = node.properties.architecture.clone().unwrap_or_default();
    Some(super::guest_packages::agent_package_programs(
        &version,
        &architecture,
    ))
}
