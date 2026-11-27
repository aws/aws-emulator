// SPDX-License-Identifier: CC0-1.0

//! The LAUNCH of an environment: every member a freshly launched environment
//! starts out carrying, and what storing one records.
//!
//! "Launches an environment for the specified application using the specified
//! configuration" is what `CreateEnvironment` does, and `ComposeEnvironments`
//! — "create or update a group of environments that each run a separate
//! component of a single application" — launches one per environment manifest
//! it was handed. The two resolve WHAT to launch from different places (one
//! from the request, the other from the manifest in a version's source bundle),
//! but a launched environment is the same resource either way: the same status
//! and health it starts in, the same address it publishes, the same resources
//! provisioned for it and the same move to `Ready` the clock owes it.
//!
//! So the record a launch produces is built HERE, from [`EnvironmentLaunch`],
//! and both operations hand their resolved values over rather than writing the
//! member list twice — two launches cannot disagree about what a new
//! environment carries.
//!
//! Split out of [`super`], which holds the store the environments live in.

use super::*;

/// What an operation that launches an environment has RESOLVED about it: the
/// identity the store minted for it, the address it publishes, and the
/// configuration it runs.
///
/// Every member is already resolved — a solution stack looked up in the
/// published catalog, a tier filled in from the model's defaults, the option
/// settings merged from whatever sources the operation draws on — so this
/// carries no request and makes no decision an operation's own contract owns.
#[derive(Debug)]
pub struct EnvironmentLaunch<'a> {
    /// The `EnvironmentId` the store minted ([`super::ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::next_environment_id`]).
    pub environment_id: String,
    pub environment_name: String,
    /// Creation order within this emulator
    /// ([`super::ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState::next_counter`]).
    pub sequence: u64,
    /// The region the launching request was addressed to.
    pub region: &'a str,
    /// WHERE this emulator publishes an environment's addresses: the zone its
    /// names are announced in and the address its instances answer at
    /// ([`super::published_addressing`]).
    pub addressing: &'a PublishedAddressing,
    /// The CNAME prefix the launch was asked for, if any: "if not specified,
    /// the CNAME is generated automatically by appending a random alphanumeric
    /// string to the environment name", which is this environment's own unique
    /// id body here, so every environment gets a distinct CNAME.
    pub cname_prefix: Option<String>,
    pub application_name: &'a str,
    pub version_label: Option<String>,
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    pub template_name: Option<String>,
    pub description: Option<String>,
    /// The tier to launch, with every member the caller left out already filled
    /// in ([`super::environment_tier_with_defaults`]).
    pub tier: Option<EnvironmentTier>,
    pub option_settings: Vec<ConfigurationOptionSetting>,
    pub operations_role: Option<String>,
    pub tags: Vec<Tag>,
    /// "A list of links to other environments in the same group", which only an
    /// environment composed from a manifest that declares them has.
    pub environment_links: Vec<EnvironmentLink>,
    /// The request this launch belongs to, which the completion event is
    /// attributed to.
    pub request_id: &'a str,
}

impl Environment {
    /// The record a launch starts the environment out with: `Launching`
    /// ("Environment is in the process of initial deployment"), reporting the
    /// default health of a new environment, with its own Amazon Web Services
    /// resources provisioned and the move to `Ready` the clock owes it.
    pub fn launching(launch: EnvironmentLaunch<'_>) -> Self {
        let EnvironmentLaunch {
            environment_id,
            environment_name,
            sequence,
            region,
            addressing,
            cname_prefix,
            application_name,
            version_label,
            solution_stack_name,
            platform_arn,
            template_name,
            description,
            tier,
            option_settings,
            operations_role,
            tags,
            environment_links,
            request_id,
        } = launch;
        let cname_prefix = cname_prefix.unwrap_or_else(|| {
            format!(
                "{environment_name}-{}",
                environment_id.trim_start_matches("e-")
            )
        });
        let created = now();
        // The NAME this environment publishes: the prefix it carries, under a
        // sub-domain of this environment's own, which is what makes the name an
        // environment answers at one label longer than the name its prefix
        // reserves ([`environment_cname`]).
        let cname = environment_cname(
            &addressing.zone,
            region,
            &new_environment_sub_domain(),
            &cname_prefix,
        );
        let mut environment = Self {
            arn: environment_arn(region, application_name, &environment_name),
            region: region.to_string(),
            application_name: application_name.to_string(),
            version_label,
            solution_stack_name,
            platform_arn,
            template_name,
            description,
            // The launch RESERVES the name it will publish, which is what
            // `CheckDNSAvailability` reports the prefix as taken for, and what a
            // second launch asking for the same prefix is refused for, from this
            // call onwards.
            reserved_cname: cname,
            // The environment is ASSIGNED that name when the provisioning this
            // launch starts has finished — not here: a launch still provisioning
            // answers at no address, so it publishes none and the reads of it
            // report no `CNAME` yet ([`Environment::materialize_due`] assigns
            // it).
            cname: String::new(),
            // Where the OTHER address it publishes is announced, and the address
            // its instances answer at: what its `EndpointURL` is derived from once
            // the resources below are provisioned ([`Environment::endpoint_url`]).
            published_zone: addressing.zone.clone(),
            instance_address: addressing.instance_address.clone(),
            tier,
            option_settings,
            // A launch deploys the configuration it was given, so there is no
            // configuration waiting to be deployed beside it: a DRAFT
            // configuration is what an UPDATE of a running environment creates.
            draft_configuration: None,
            operations_role,
            // "Launching: Environment is in the process of initial deployment."
            status: STATUS_LAUNCHING.to_string(),
            // "Grey: Default health for a new environment."
            health: HEALTH_GREY.to_string(),
            health_status: Some(HEALTH_STATUS_PENDING.to_string()),
            abortable_operation_in_progress: false,
            date_created: created,
            date_updated: created,
            date_terminated: None,
            tags,
            sequence,
            // The launch provisions the instances below, which is when the week
            // until a managed replacement of them is due starts; the field is
            // dated with the launch so the record never carries no date at all.
            instances_replaced_at: created,
            // The process those instances run the application as: the launch
            // starts the FIRST one, and the number moves when it is replaced
            // ([`Environment::app_process_generation`]).
            app_process_generation: 1,
            // Nothing has dialled this environment's address yet, so its
            // application has answered no request for the health agent to report
            // on ([`super::served_requests`]).
            served_answers: Vec::new(),
            // "This number increases by one each time that you deploy source
            // code or change instance configuration settings": the launch is
            // this environment's first deployment, which is what its instances
            // report running.
            deployment_id: 1,
            // Provisioned below, from the tier and the options this record now
            // carries, so the inventory is derived from the stored environment
            // rather than from the launch a second time.
            resources: None,
            // The launch the clock owes: `Launching` -> `Ready`.
            pending_status: Some(PendingStatus::launch_completion(
                request_id,
                &environment_name,
            )),
            environment_links,
            environment_id,
            environment_name,
        };
        // "Launches an environment for the specified application using the
        // specified configuration": the launch provisions the environment's own
        // Amazon Web Services resources — its Auto Scaling group, the launch
        // template it launches instances from, those instances, and the load
        // balancer or the worker queue its tier and its options call for —
        // which is what `DescribeEnvironmentResources` then reports.
        environment.provision_resources();
        environment
    }
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Store a freshly launched environment under both the keys an operation
    /// resolves one by, and record that the launching call's activity has
    /// STARTED.
    ///
    /// The completion event is recorded when the clock makes the move to
    /// `Ready` due, so what is recorded here is only that the launch began,
    /// under the request that asked for it. `operation_name` is the operation
    /// performing the launch, as the service names it in that message, since a
    /// client follows the launch through the call that asked for it.
    pub fn store_launched_environment(
        &mut self,
        environment: Environment,
        request_id: &str,
        operation_name: &str,
    ) {
        let subject = environment.event_subject();
        self.environment_name_index.insert(
            environment.environment_name.clone(),
            environment.environment_id.clone(),
        );
        self.environments
            .insert(environment.environment_id.clone(), environment);
        self.record_activity_start(request_id, operation_name, subject);
    }
}
