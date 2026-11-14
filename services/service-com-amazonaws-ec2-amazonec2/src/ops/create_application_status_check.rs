// SPDX-License-Identifier: CC0-1.0

//! @op CreateApplicationStatusCheck
//!
//! Creates one application status check from the configuration the request names and STORES
//! it ([`crate::state::ComAmazonawsEc2Amazonec2State::application_status_checks`]), so the
//! check it answers with is the one `DescribeApplicationStatusChecks` reports afterwards.
//!
//! Every member the request leaves out takes the value AWS documents as its default, and
//! every published bound the model carries no trait for — the one valid `Interval`, the
//! `Timeout` range and its relation to `Interval`, the leading slash of `Path`, the form and
//! length of `StatusCodeMatcher`, the thresholds above zero and the account's quota of checks
//! — is enforced here before anything is written. Nothing about the check is derived from the
//! clock except its creation instant: the health checks themselves do not start until a
//! caller associates it ("You must associate the check with instances or tags using
//! AssociateApplicationStatusCheck before health checks start"), so a fresh check reports no
//! results, no associations and no modification.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store is
//! reached through [`crate::state::require_service_state`], the way every handler beside this
//! one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateApplicationStatusCheck`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Creates an application status check for monitoring the health of applications running on
//! your instances. You can configure the protocol, port, path, and thresholds for the
//! health check. The following rules apply: You can create a maximum of 50 application
//! status checks for each account. You must associate the check with instances or tags
//! using AssociateApplicationStatusCheck before health checks start. You must set the
//! Timeout value to less than the Interval value. You must start the Path with a forward
//! slash (/). Default: /. You can specify Aggregation as included or excluded. If you do
//! not specify a value, it defaults to included, which means the check contributes to the
//! instance-level application status. You can use the following default values: Interval is
//! 60 seconds, Timeout is 6 seconds, FailureThreshold is 2, SuccessThreshold is 2,
//! StatusCodeMatcher is 200, InitializationGracePeriodSeconds is 300 seconds. You can tag
//! the application status check during creation. For more information, see Tag your Amazon
//! EC2 resources.
//!
//! ## Input
//!
//! * `Aggregation` (AggregationStatusEnum, one of: excluded | included) — The aggregation
//!   setting for the application status check. When set to included, the result of this check
//!   contributes to the instance-level application status reported by
//!   DescribeApplicationStatus. When set to excluded, the check runs independently and does
//!   not affect the instance-level status. Valid values: included | excluded.
//! * `ClientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier that you
//!   provide to ensure that the operation completes no more than one time. If you retry a
//!   request with the same token, the service ignores the request but does not return an
//!   error. For more information, see Ensuring idempotency.
//! * `DeviceIndex` (Integer) — The index of the network device to use for the health check.
//!   The value must be greater than or equal to 0.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the operation,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `FailureThreshold` (Integer) — The number of consecutive failed health checks before the
//!   application status is considered impaired. The value must be greater than 0.
//! * `HealthCheckPaths` (HealthCheckPathRequestList) — The health check paths to use for the
//!   application status check. Health check paths define the network path from a source
//!   subnet to one or more destination subnets for cross-Availability Zone or Availability
//!   Zone to Local Zone health checking. If omitted, health checks are performed in the same
//!   subnet as the instance.
//! * `InitializationGracePeriodSeconds` (InitializationGracePeriodSeconds, range -1..600) —
//!   The number of seconds to wait before starting health checks after an instance is
//!   launched. Valid values: 1 to 600.
//! * `Interval` (Integer) — The interval, in seconds, between health checks. Valid value: 60.
//! * `IpScope` (IpScopeEnum, one of: private) — The IP scope to use for the health check.
//!   Valid value: private.
//! * `IpVersion` (IpVersionEnum, one of: ipv4 | ipv6) — The IP version to use for the health
//!   check. Valid values: ipv4 and ipv6.
//! * `Path` (String) — The URL path to use for the health check HTTP request (for example,
//!   /health or /status).
//! * `Port` (PortNumber, range 1..65535) **required** — The port to use for the health check.
//!   Valid values: 1 to 65535.
//! * `Protocol` (NetworkProtocolEnum, one of: http | https) **required** — The protocol to
//!   use for the health check. Valid values: http | https.
//! * `StatusCodeMatcher` (String) — The HTTP status codes that indicate a successful health
//!   check response. Specify a comma-separated list of individual status codes or ranges, for
//!   example, 200,202,300-399. For a range, the first value must be less than the second
//!   value. Maximum length: 64 characters. Default: 200.
//! * `SuccessThreshold` (Integer) — The number of consecutive successful health checks before
//!   the application status is considered healthy. The value must be greater than 0.
//! * `TagSpecifications` (TagSpecificationList) — The tags to apply to the application status
//!   check.
//! * `Timeout` (Integer) — The amount of time, in seconds, to wait for a health check
//!   response before considering it failed. Valid values: 1 to 30. The value must be less
//!   than Interval.
//!
//! ## Output
//!
//! * `ApplicationStatusCheck` (ApplicationStatusCheckResponseObject) — Information about the
//!   application status check.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateApplicationStatusCheckError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateApplicationStatusCheckError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#CreateApplicationStatusCheck")]
#[derive(Debug)]
pub enum CreateApplicationStatusCheckErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_application_status_check::CreateApplicationStatusCheckError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateApplicationStatusCheckError = crate::scaffolded::Scaffolded<CreateApplicationStatusCheckErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateApplicationStatusCheckErrorModeled> for CreateApplicationStatusCheckError {
    fn from(modeled: CreateApplicationStatusCheckErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateApplicationStatusCheck` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateApplicationStatusCheck<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateApplicationStatusCheck<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateApplicationStatusCheck<Eng>
{
    type Input = CreateApplicationStatusCheckInput;
    type Output = CreateApplicationStatusCheckOutput;
    type Error = CreateApplicationStatusCheckError;

    const NAME: &'static str = "CreateApplicationStatusCheck";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_STATUS_CHECK_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_APPLICATION_STATUS_CHECK_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use ::std::time::SystemTime;

        use crate::scaffolded::Scaffolded;
        use crate::state::{
            active_application_status_check_count, application_status_check_limit_exceeded,
            application_status_check_payload_bytes, health_check_paths_from_request,
            idempotent_parameter_mismatch,
            invalid_group_not_found, invalid_subnet_id_not_found, require_service_state,
            same_application_status_check_configuration, status_code_matcher_is_well_formed,
            system_time_to_date_time, tags_from_specifications, AGGREGATION_INCLUDED,
            APPLICATION_STATUS_CHECK_ID_PREFIX, APPLICATION_STATUS_CHECK_INTERVAL_SECONDS,
            APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE, APPLICATION_STATUS_CHECK_TIMEOUT_RANGE,
            DEFAULT_APPLICATION_STATUS_CHECK_FAILURE_THRESHOLD,
            DEFAULT_APPLICATION_STATUS_CHECK_DEVICE_INDEX,
            DEFAULT_APPLICATION_STATUS_CHECK_GRACE_PERIOD_SECONDS,
            DEFAULT_APPLICATION_STATUS_CHECK_IP_SCOPE,
            DEFAULT_APPLICATION_STATUS_CHECK_IP_VERSION,
            DEFAULT_APPLICATION_STATUS_CHECK_PATH,
            DEFAULT_APPLICATION_STATUS_CHECK_STATUS_CODE_MATCHER,
            DEFAULT_APPLICATION_STATUS_CHECK_SUCCESS_THRESHOLD,
            DEFAULT_APPLICATION_STATUS_CHECK_TIMEOUT, MAX_APPLICATION_STATUS_CHECKS,
            MAX_CLIENT_TOKEN_CHARS, MAX_HEALTH_CHECK_PATH_CHARS,
            MAX_STATUS_CODE_MATCHER_CHARS, MAX_STORED_BYTES,
        };

        // `ctx` carries request metadata only; this operation reads none of it — a check's
        // configuration is a fact of the check rather than of the Region the request was
        // signed for, which is also how `DescribeApplicationStatusChecks` reads one back.
        // The store is reached through the engine held on `self`.
        let _ = ctx;

        // "A unique, case-sensitive identifier that you provide to ensure that the operation
        // completes no more than one time" (`$ClientToken`): the token map keeps whatever it
        // is given for the process's lifetime, so its length is bounded the way every other
        // create in this crate bounds it.
        if input
            .client_token
            .as_ref()
            .is_some_and(|token| token.chars().count() > MAX_CLIENT_TOKEN_CHARS)
        {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "clientToken accepts at most {MAX_CLIENT_TOKEN_CHARS} characters"
            )));
        }

        // "You can specify Aggregation as included or excluded. If you do not specify a
        // value, it defaults to included": the modelled enum has already refused any other
        // spelling, so the only choice left here is what an absent member means.
        let aggregation = input
            .aggregation
            .clone()
            .unwrap_or_else(|| AGGREGATION_INCLUDED.to_string());

        // "You must start the Path with a forward slash (/). Default: /" — and the member is
        // a plain `String` the model bounds nowhere, so the bytes one check can retain are
        // bounded here too.
        let path = input
            .path
            .clone()
            .unwrap_or_else(|| DEFAULT_APPLICATION_STATUS_CHECK_PATH.to_string());
        if !path.starts_with('/') {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{path}' for path. The path must start with a forward slash (/)"
            )));
        }
        if path.chars().count() > MAX_HEALTH_CHECK_PATH_CHARS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "path accepts at most {MAX_HEALTH_CHECK_PATH_CHARS} characters"
            )));
        }

        // "The interval, in seconds, between health checks. Valid value: 60" (`$Interval`),
        // which is also the interval a request naming none gets ("Interval is 60 seconds").
        // The member carries no modelled `@range`, so this is the only place that published
        // value is enforced.
        let interval = input
            .interval
            .unwrap_or(APPLICATION_STATUS_CHECK_INTERVAL_SECONDS);
        if interval != APPLICATION_STATUS_CHECK_INTERVAL_SECONDS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{interval}' for interval. The only valid value is \
                 {APPLICATION_STATUS_CHECK_INTERVAL_SECONDS}"
            )));
        }

        // "The amount of time, in seconds, to wait for a health check response before
        // considering it failed. Valid values: 1 to 30" (`$Timeout`). Absent is "Timeout is 6
        // seconds". The operation's other sentence about it — "You must set the Timeout value
        // to less than the Interval value" — holds for every pair those two published bounds
        // admit, the one valid interval being 60 and the largest timeout 30, so there is
        // nothing left for a second comparison to refuse.
        let (timeout_min, timeout_max) = APPLICATION_STATUS_CHECK_TIMEOUT_RANGE;
        let timeout = input
            .timeout
            .unwrap_or(DEFAULT_APPLICATION_STATUS_CHECK_TIMEOUT);
        if timeout < timeout_min || timeout > timeout_max {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{timeout}' for timeout. It must be from {timeout_min} to \
                 {timeout_max}"
            )));
        }

        // "The number of consecutive failed health checks before the application status is
        // considered impaired. The value must be greater than 0" (`$FailureThreshold`), and
        // the same sentence for `$SuccessThreshold`. Absent is "FailureThreshold is 2,
        // SuccessThreshold is 2".
        let failure_threshold = input
            .failure_threshold
            .unwrap_or(DEFAULT_APPLICATION_STATUS_CHECK_FAILURE_THRESHOLD);
        if failure_threshold <= 0 {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{failure_threshold}' for failureThreshold. It must be greater \
                 than 0"
            )));
        }
        let success_threshold = input
            .success_threshold
            .unwrap_or(DEFAULT_APPLICATION_STATUS_CHECK_SUCCESS_THRESHOLD);
        if success_threshold <= 0 {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{success_threshold}' for successThreshold. It must be greater \
                 than 0"
            )));
        }

        // "The index of the network device to use for the health check. The value must be
        // greater than or equal to 0" (`$DeviceIndex`), a plain `Integer` with no modelled
        // `@range`, so this is the only place that published bound is enforced. A request
        // naming none gets the device every instance has
        // ([`DEFAULT_APPLICATION_STATUS_CHECK_DEVICE_INDEX`]), which the created check reports
        // like every other member whose value the request left to the service.
        let device_index = input
            .device_index
            .unwrap_or(DEFAULT_APPLICATION_STATUS_CHECK_DEVICE_INDEX);
        if device_index < 0 {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{device_index}' for deviceIndex. It must be greater than or \
                 equal to 0"
            )));
        }

        // "The IP version to use for the health check. Valid values: ipv4 and ipv6"
        // (`$IpVersion`) and "The IP scope to use for the health check. Valid value: private"
        // (`$IpScope`): the modelled enums have already refused any other spelling, so the
        // only choice left here is what an absent member means — the version and scope of the
        // address a health check reaches an instance on by default.
        let ip_version = input
            .ip_version
            .clone()
            .unwrap_or_else(|| DEFAULT_APPLICATION_STATUS_CHECK_IP_VERSION.to_string());
        let ip_scope = input
            .ip_scope
            .clone()
            .unwrap_or_else(|| DEFAULT_APPLICATION_STATUS_CHECK_IP_SCOPE.to_string());

        // "Specify a comma-separated list of individual status codes or ranges, for example,
        // 200,202,300-399. For a range, the first value must be less than the second value.
        // Maximum length: 64 characters. Default: 200" (`$StatusCodeMatcher`).
        let status_code_matcher = input
            .status_code_matcher
            .clone()
            .unwrap_or_else(|| DEFAULT_APPLICATION_STATUS_CHECK_STATUS_CODE_MATCHER.to_string());
        if status_code_matcher.chars().count() > MAX_STATUS_CODE_MATCHER_CHARS {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "statusCodeMatcher accepts at most {MAX_STATUS_CODE_MATCHER_CHARS} characters"
            )));
        }
        if !status_code_matcher_is_well_formed(&status_code_matcher) {
            return Err(Scaffolded::invalid_parameter_value(format!(
                "Invalid value '{status_code_matcher}' for statusCodeMatcher. It must be a \
                 comma-separated list of HTTP status codes or ranges, and the first value of a \
                 range must be less than the second"
            )));
        }

        // "The number of seconds to wait before starting health checks after an instance is
        // launched" (`$InitializationGracePeriodSeconds`), whose modelled `@range` of -1..600
        // the boundary validator has already enforced — so no second bound is read here.
        // Absent is "InitializationGracePeriodSeconds is 300 seconds".
        let initialization_grace_period_seconds = input
            .initialization_grace_period_seconds
            .unwrap_or(DEFAULT_APPLICATION_STATUS_CHECK_GRACE_PERIOD_SECONDS);

        // "Health check paths define the network path from a source subnet to one or more
        // destination subnets... If omitted, health checks are performed in the same subnet
        // as the instance" — so a request naming no paths at all asks for the in-subnet
        // check, which is no paths. The reading of the member, and the bounds on it, are the
        // ONE this crate keeps for it ([`crate::state::health_check_paths_from_request`]), so
        // the modify that replaces a check's paths reads them exactly as this create does.
        let health_check_paths =
            health_check_paths_from_request(input.health_check_paths.as_deref().unwrap_or_default())?;

        // "The tags to apply to the application status check" (`$TagSpecifications`): the
        // check is the only resource this call creates, so a specification naming any other
        // resource type is refused rather than accepted and discarded — by the one helper
        // every create in this crate bounds its tag payload with.
        let tags = tags_from_specifications(
            input.tag_specifications.as_deref(),
            APPLICATION_STATUS_CHECK_TAGGABLE_RESOURCE,
            Self::NAME,
        )?;

        // The check as it will be STORED and reported, every documented default filled in, so
        // the response, the store and a later describe cannot disagree about one member. The
        // id and the creation instant are the service's to decide and are set at the write
        // below; the three remaining instants belong to events that have not happened —
        // nothing has modified the check, nothing has deleted it, and no health check has run
        // for it, there being no association yet.
        let mut check = ApplicationStatusCheckResponseObject {
            application_status_check_id: None,
            aggregation: Some(aggregation),
            health_check_paths: Some(health_check_paths),
            protocol: Some(input.protocol.clone()),
            port: Some(input.port),
            path: Some(path),
            device_index: Some(device_index),
            ip_version: Some(ip_version),
            ip_scope: Some(ip_scope),
            interval: Some(interval),
            timeout: Some(timeout),
            failure_threshold: Some(failure_threshold),
            success_threshold: Some(success_threshold),
            status_code_matcher: Some(status_code_matcher),
            initialization_grace_period_seconds: Some(initialization_grace_period_seconds),
            last_updated_at: None,
            // "Instances with these tags are automatically monitored by this check"
            // (`$TargetTagAssociations`): a check nobody has associated yet is monitoring
            // nothing, which is an empty set rather than an invented association.
            target_tag_associations: Some(Vec::new()),
            tags: Some(tags),
            creation_time: None,
            modify_time: None,
            deletion_time: None,
        };

        // The caller-chosen bytes this create would retain for the process's lifetime: the
        // tags, the path, the matcher, the ids its health check paths name, and the client
        // token the token map keeps. A COUNT ceiling admits megabytes on its own, because the
        // record holds strings the caller chose the length of, so both are asked below.
        let payload_bytes =
            application_status_check_payload_bytes(&check, input.client_token.as_deref());

        let state = require_service_state(&*self.engine)?;
        let mut store = state.lock();

        // Every subnet and security group a health check path names must be one the account
        // holds: a path through a subnet that does not exist is not a path, and this is the
        // same answer every other create of this crate gives for an id it cannot resolve.
        for path in check.health_check_paths.as_deref().unwrap_or_default() {
            let source = path.source.as_ref();
            let subnet_ids = source
                .and_then(|source| source.subnet_id.as_deref())
                .into_iter()
                .chain(
                    path.destinations
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|destination| destination.subnet_id.as_deref()),
                );
            for subnet_id in subnet_ids {
                if !store.subnets.contains_key(subnet_id) {
                    return Err(invalid_subnet_id_not_found(subnet_id));
                }
            }
            let group_ids = source
                .and_then(|source| source.security_group_id.as_deref())
                .into_iter()
                .chain(
                    path.destinations
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|destination| destination.security_group_id.as_deref()),
                );
            for group_id in group_ids {
                if !store.security_groups.contains_key(group_id) {
                    return Err(invalid_group_not_found(group_id));
                }
            }
        }

        // Idempotency: one token names the one check it created, so a replay reports that
        // check and creates nothing. Read here, answered below the DryRun check, because a
        // replay stores nothing and so passes the ceilings a fresh create has to clear.
        let replayed = input
            .client_token
            .as_deref()
            .and_then(|token| store.application_status_check_client_tokens.get(token).cloned());

        // A token is only idempotent for the REQUEST it was first given with: replayed with a
        // different configuration it names a check the caller did not ask for, which EC2
        // refuses rather than answers. Checked before the DryRun flag, so a dry run of such a
        // request is refused with this reason too.
        if let Some(application_status_check_id) = replayed.as_deref() {
            let stored = store
                .application_status_checks
                .get(application_status_check_id)
                .ok_or_else(|| {
                    Scaffolded::internal_error(format!(
                        "a client token names application status check \
                         '{application_status_check_id}', which is not stored"
                    ))
                })?;
            if !same_application_status_check_configuration(stored, &check) {
                return Err(idempotent_parameter_mismatch());
            }
        }

        // "You can create a maximum of 50 application status checks for each account": asked
        // of the ACTIVE checks, and only of a request that would store one.
        if replayed.is_none() {
            let held = active_application_status_check_count(&store);
            if held >= MAX_APPLICATION_STATUS_CHECKS {
                return Err(application_status_check_limit_exceeded(held));
            }
        }
        if replayed.is_none() && store.would_exceed_stored_bytes(payload_bytes) {
            return Err(Scaffolded::request_limit_exceeded(format!(
                "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                 caller-supplied data, so it cannot store {payload_bytes} more",
                store.stored_bytes()
            )));
        }

        // DryRun sits AFTER every parameter has been validated and BEFORE the first write:
        // the flag asks whether the request WOULD have worked, so one that would have been
        // refused is refused with that reason instead, and one that would have succeeded
        // leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(Scaffolded::dry_run_operation());
        }

        if let Some(application_status_check_id) = replayed {
            return store
                .application_status_checks
                .get(&application_status_check_id)
                .map(|stored| CreateApplicationStatusCheckOutput {
                    application_status_check: Some(stored.clone()),
                })
                .ok_or_else(|| {
                    Scaffolded::internal_error(format!(
                        "a client token names application status check \
                         '{application_status_check_id}', which is not stored"
                    ))
                });
        }

        let application_status_check_id = store.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
        check.application_status_check_id = Some(application_status_check_id.clone());
        // "The date and time when the application status check was created"
        // (`$CreationTime`) — this call is that event, so the instant is read off the clock
        // here and never again.
        check.creation_time = Some(system_time_to_date_time(SystemTime::now()));
        if let Some(token) = input.client_token.clone() {
            store
                .application_status_check_client_tokens
                .insert(token, application_status_check_id.clone());
        }
        // The bytes this record and its token retain are charged against the store's budget
        // at the insert, so no path can store them uncharged; a removal path must give them
        // back with `release_stored_bytes`.
        store.add_stored_bytes(payload_bytes);
        store
            .application_status_checks
            .insert(application_status_check_id.clone(), check);

        // Read the response back OUT of the store, so the caller sees what a later
        // `DescribeApplicationStatusChecks` will see rather than the request object.
        store
            .application_status_checks
            .get(&application_status_check_id)
            .map(|stored| CreateApplicationStatusCheckOutput {
                application_status_check: Some(stored.clone()),
            })
            .ok_or_else(|| {
                Scaffolded::internal_error(format!(
                    "the newly created application status check \
                     '{application_status_check_id}' could not be read back"
                ))
            })
    }
}
