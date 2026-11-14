// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
//! Code more than one emulated service needs, as a path dependency.
//!
//! Today that is eight things:
//!
//! * [`claims_request`], the request-claim rule a service whose SigV4 signing name is
//!   SHARED with other services uses for its `Service::can_handle`;
//! * [`claims_modeled_route`], the REST form of that rule at full strength — the WHOLE
//!   modelled route, method AND URI template — which a REST service under a shared
//!   signing name should reach for by DEFAULT, keeping [`claims_modeled_path`]'s
//!   first-segment form only where the service's own route table is out of reach;
//! * [`arn`], the one parse of a caller-supplied resource ARN, so no two services
//!   disagree about which vendor an ARN names;
//! * [`nth_path_segment`] and [`addresses_modeled_route`], for the rarer case of two such
//!   services publishing the SAME path space — the segment their modelled URIs differ at,
//!   or (where only the whole route tells them apart) whether a service's own model binds
//!   a route for the request at all. [`bedrock`] states that split once for the six
//!   services that share the signing name `bedrock`, [`lex`] for the four that share
//!   `lex`, and the two AgentCore endpoints that share `bedrock-agentcore` call
//!   [`claims_modeled_route`], which is that route test under the routing-key one;
//! * [`shared_path`], the reader for the one case the path half of the claim rule
//!   cannot settle on its own: two colliding models binding the same URI path space,
//!   where the ARN in the path names the resource and so the service that owns it;
//! * [`addresses_modeled_http_route`], the other side of [`addresses_modeled_route`]: which
//!   operation a request ADDRESSES — the method and URI pattern of one operation's own
//!   `@http` binding — for a service answering a request the framework's route lookup
//!   withheld a route from because a `@required` `@httpQuery` member was absent;
//! * [`chime_family`], the same claim rule for the one family whose shared signing name
//!   is not even its members' endpoint prefix: the six Amazon Chime services, which are
//!   told apart by the endpoint the request addresses and then by their own path spaces;
//! * [`front_door`], the body-rejection layer an AWS REST service puts in front of its
//!   application code, as a wrapper a service serves its operations behind;
//! * [`validation`], which restores the value a constraint violation echoes to the one
//!   the caller submitted, so a service wording its own `ValidationException` from the
//!   framework's violations does not report a value nobody sent;
//! * [`iam_trust_policy`], the ONE reading of an IAM trust policy, for every service that
//!   resolves a caller-supplied execution role before it writes — so two services asked
//!   whether a document lets them assume a role cannot answer differently;
//! * [`iam_authorization`], the ONE evaluation of IAM policy documents against a request,
//!   for every service that decides server-side whether the calling principal may perform
//!   an action on a resource — so two services handed the same identity-based and
//!   resource-based documents reach the same decision;
//! * [`stable_hex_id`], the one deterministic mixing function a service derives a
//!   wire-visible id with;
//! * [`uuid_from_random_bytes`], the one rendering of 16 opaque bytes as a version-4
//!   UUID, for every service that mints an identifier in that form — so the version
//!   and variant bits and the hyphenated text form are written in one place;
//! * [`network`], the contract the account's EC2 network — which subnets a Region has —
//!   is modelled against, so no two services answer "does this subnet exist"
//!   differently;
//! * [`rfc3339`], the ONE reading of the UTC offset an RFC 3339 timestamp declares, for
//!   every service whose model carries a timestamp as a STRING and then has to compare or
//!   order two of them;
//! * [`rpc_target`], the ONE reading of an `awsJson` `X-Amz-Target` — which service and
//!   which operation it names — for every service whose published target prefix carries
//!   dots of its own, which the framework's first-dot readers cannot route;
//!
//! Beside them, ten SHARED STORES live here, each held once in the shared engine so
//! the services on either side of a hand-off cannot hold two copies of one fact:
//!
//! * [`acm_certificate_associations`], the directory of which resources reference an ACM
//!   certificate — written by the service that owns the referencing resource (a custom
//!   domain name, a listener) and read by ACM, which cannot see those resources and yet
//!   has to report them and refuse to delete or revoke a certificate while one remains;
//! * [`acm_certificate_key_material`], the SERVING material of an ACM certificate — the
//!   leaf, its chain and the private key that matches it — written by ACM when a caller
//!   imports key material into it and read by the service that TERMINATES a connection
//!   presenting that certificate (a load balancer listener), which cannot see ACM's store
//!   and cannot run a handshake without the certificate's own bytes and its key;
//! * [`iot_job_executions`], the IoT job-execution store the IoT control plane writes
//!   (a job targets a thing) and the IoT Jobs data plane reads and updates (the device
//!   polls, starts and reports);
//! * [`iot_jobs`], the account's IoT jobs — written by every control plane that mints
//!   one (IoT's own job and OTA-update creates, and the IoT Greengrass V2 deployment
//!   create, which reports the id and ARN of the job that applies the deployment) and
//!   read by the IoT jobs plane, which is where a client then describes, deletes or
//!   waits on that job and so must find the very job the other service reported;
//! * [`db_data_sources`], the directory of which database service minted an immutable
//!   DB resource id — written by the database control planes when they mint one, read
//!   by the services that monitor those resources, which cannot tell an RDS id from a
//!   DocumentDB id by its form;
//! * [`agentic_rft_jobs`], the SageMaker job directory the SageMaker control plane writes
//!   (a create mints a job of a reinforcement-fine-tuning category) and the AgenticRFT job
//!   runtime data plane reads — whose own API carries no job definition and no call that
//!   creates one, so whether the job ARN a sampling request names exists at all is
//!   knowable there only because the control plane recorded it;
//! * [`feature_store_groups`], the SageMaker Feature Store feature-group directory the
//!   SageMaker control plane writes (`CreateFeatureGroup` defines a group, its schema
//!   and its two key feature names) and the FeatureStore Runtime data plane reads — whose
//!   own API carries no feature-group definition, so a record's identifier feature and
//!   its event-time feature, and whether the named group exists at all, are knowable
//!   there only because the control plane recorded them;
//! * [`management_events`], the account's management-event (audit) log — written by the
//!   service that SERVED an API call, which is the only one that knows the call
//!   happened, and read by CloudTrail, which is the service that reports it
//!   (`LookupEvents`) and, while a trail of the Region is logging, fanned out onto the
//!   account's default event bus ([`api_call_events`]);
//! * [`chime_identity_names`], the name an Amazon Chime SDK app instance's identity is
//!   registered under, keyed by its ARN — written by Chime SDK Identity, which owns the
//!   identity, and read by Chime SDK Messaging, which reports every participant of a
//!   channel as an ARN and that name and cannot see the store the identity lives in;
//! * [`ses_identities`], the account's email identity registry — the ONE set of
//!   identities Amazon SES's two APIs publish, written by whichever surface records or
//!   deletes an identity and read by both, so an identity created with the modern API
//!   is listed, reported and gated on by the legacy one and the other way round;
//! * [`ses_configuration_sets`], the account's configuration-set registry — the ONE set of
//!   configuration sets those same two APIs publish, written by whichever surface records,
//!   configures or deletes a set and read by both, so a set created with either API is
//!   described, used on a send and deleted through the other;
//! * [`service_principal_calls`], the cross-service calls one service is making as its own
//!   SERVICE PRINCIPAL on behalf of one of its resources — written by the calling service
//!   while its call is in flight and read by the service that authorizes that call, which
//!   cannot read the caller's store and cannot read the principal off a credential every
//!   in-process call shares;
//! * [`ad_directories`], the inventory of which Active Directory directories the account
//!   holds and what the control plane knows about each — written by the AWS Directory
//!   Service control plane, which creates and deletes them, names their domains, switches
//!   their data access on and off and sets their accounts' passwords, and read by the
//!   Directory Service DATA plane, whose own API creates no directory and so can tell
//!   neither a directory of this account from one that is not, nor the realm its objects
//!   live in, nor which of its accounts hold a password;
//! * [`arc_managed_resources`], the resources registered with Amazon Application Recovery
//!   Controller for zonal shift — written by the service that OWNS the resource, because
//!   ARC models no operation that registers one, and read by ARC, which reports a managed
//!   resource, its zones and the shifts in progress for it and cannot see the store that
//!   resource lives in.
//!
//! # Why a shared rule
//!
//! The router resolves a request's routing key from the SigV4 credential scope
//! first, which carries the SIGNING name — and several AWS services sign under one
//! name while serving distinct endpoints. Every AWS Marketplace-family service signs
//! as `aws-marketplace` (Marketplace Catalog, Reporting, Deployment, Discovery,
//! Agreement, Entitlement, Metering), `iam` is shared, and so is `sagemaker` (nine
//! services: the control plane plus the metrics, a2i-runtime, edge,
//! featurestore-runtime, job-runtime, training-session-runtime and two runtime
//! endpoints). A service that claims on the signing name alone therefore claims its
//! peers' traffic too, every claimant is a claimant for every request, and the
//! router — which refuses to pick one — answers `500 AmbiguousService` to all of them.
//!
//! The rule below is the fix, stated once: a service claims a request when the
//! signing name matches AND the request addresses that service's OWN modelled wire
//! surface. What "addresses" means follows the protocol the model declares, and
//! nothing here names a service:
//!
//! * A REST protocol (`restJson1`, `restXml`) binds each operation to a URI, so the
//!   request must address a URI this service's model binds. The framework's own
//!   discriminator, [`claims_modeled_path`], compares just the FIRST path segment; when
//!   two peers' path spaces are not disjoint that segment separates nothing, and the
//!   whole route — method and URI template, from the service's own generated route
//!   table — decides instead: [`claims_modeled_route`], the default for a REST service
//!   here.
//! * An RPC protocol carries the target service out of band and posts every
//!   operation to one path: `awsJson1_0` / `awsJson1_1` name it in
//!   `X-Amz-Target: <Service>.<Operation>`, and `rpcv2Cbor` in the URI
//!   `/service/<Service>/operation/<Operation>`. So the service NAMED there must be
//!   this one.
//!
//! Path segments alone cannot separate RPC peers: every one of them posts to `/`,
//! so the generated `ROUTE_PATH_SEGMENTS` is empty (or just the two protocol
//! sentinels) for all of them and they all keep claiming everything. The RPC
//! conjunct is what separates them.
//!
//! Two API VERSIONS of one service are the sharpest case of that: AWS publishes both
//! behind one endpoint and one signing name (Kinesis Analytics' 2015-08-14 and
//! 2018-05-23 services both carry `endpointPrefix`/`sigv4Name` `kinesisanalytics`), so
//! their shape names differ ONLY by the trailing `_<digits>` API-version suffix — the
//! suffix the framework's routing-key comparison drops. [`claims_versioned_rpc_request`]
//! is the RPC conjunct with that suffix KEPT, so each version claims its own targets and
//! neither claims its peer's.
//!
//! When two REST peers' path spaces overlap at EVERY segment a prefix test can read, no
//! segment index separates them either. What does is the whole modelled route: AWS
//! dispatches a REST request on the endpoint host and then on the operation's method +
//! URI, so a peer claims exactly the requests its OWN model binds a route for — the key
//! half and that route half together are [`claims_modeled_route`].
//!
//! Two REST peers can also publish the same first path segment — the two SageMaker
//! Runtime endpoints both bind `/endpoints/{EndpointName}/…` and even share an
//! `endpointPrefix` — and then the first segment separates nothing either. Where the
//! overlap is one operation wide, the segment their modelled URIs actually differ at
//! decides, which is what [`nth_path_segment`] reads and what
//! [`addresses_runtime_bidirectional_stream`] states once for that pair: the service
//! that owns the narrower space claims it, its peer claims the complement, so a path
//! inside the shared space always has exactly one claimant.
//!
//! Where the overlap is the WHOLE first segment — the four Amazon Lex services all sign
//! as `lex`, and three of them bind operations under `/bots/…` — no single segment
//! separates them and [`lex::claims_request`] is the rule instead: on a segment a sibling
//! publishes too, a service claims a request only when its own model binds a route for
//! that request's HTTP method and URI ([`addresses_modeled_route`]), and where two models
//! bind the IDENTICAL route the modelled shape of the path label decides. The route is
//! the finest discriminator a REST model publishes, it names no service, and it is read
//! through the service's OWN protocol (`routing_key`), so the claim and the later
//! operation lookup cannot disagree. The six services that sign as `chime` are the other
//! instance of that shape, and their shared signing name is not even their endpoint
//! prefix, so they are told apart by [`chime_family`] instead.
//!
//! The two IoT Greengrass API versions are that shape taken to its limit: one endpoint,
//! one signing name, one `arnNamespace`, and the SAME two first path segments for every
//! route either of them binds. [`greengrass::claims_request`] is the rule there — the
//! whole modelled route, narrowed for the eight routes the two models bind IDENTICALLY:
//! the account- and thing-level facts both versions publish go to one declared owner, and
//! a tagging request to the version that owns the resource its ARN names.
//!
//! The four `lambda`-signing services are the other instance of that shape: the Lambda
//! control plane, Lambda Core, Lambda Web and Lambda MicroVMs each bind their own dated
//! URI prefixes, so the first segment separates all of them EXCEPT the tagging space
//! `/2017-03-31/tags/{Resource}`, which the control plane and MicroVMs both bind; there
//! the ARN in the path decides, which is what [`shared_path`] reads for both sides.
//!
//! When the two models' path spaces overlap at MANY segments, neither the first segment
//! nor one deeper segment separates them: AgentCore's data plane and control plane share
//! nine first path segments and differ, per operation, at a different depth of each. For
//! that case [`addresses_modeled_route`] asks the framework's own route matcher
//! (`ServerProtocol::routing_key`) whether a service's model binds a route for the
//! request's METHOD + URI — the granularity a REST protocol actually routes at — so each
//! peer claims exactly its own modelled routes and no literal segment catalogue is
//! written anywhere.
//!
//! # Services still carrying their own copy
//!
//! The path half of the rule predates this crate and is inlined in the `can_handle` of
//! the service crates that needed it before there was anywhere shared to put it —
//! `grep -rln 'fn claims_modeled_path' services/*/src/service.rs` lists them (today:
//! `connect`, `connectcontactlens`, `iam`, `iamtoolbox`,
//! `personalizeevents`, `personalizeruntime`, `signerdata`, and the marketplace family
//! before this crate existed; `s3` carries only `first_path_segment`, and `appconfig`,
//! `appconfigdata`, `ivs` and `ivsrealtime` carry a `can_handle_parts` shim that calls the
//! framework helper on a rebuilt request; `chimesdkmeetings` carried one until the Chime
//! family moved onto [`chime_family`]). Those copies are to be MIGRATED onto
//! [`claims_request`] (they are REST-only cases, so the call is
//! `claims_request(parts, signing_name, crate::ROUTE_PATH_SEGMENTS, None)`); they are
//! left in place here only because this change does not own those services' issues and
//! each is covered by its own routing suite under `core/router/tests/all/`. Nothing new
//! should inline the rule again — call this crate.
//!
//! # Beside the claim rule
//!
//! [`rds_cluster_parameters`] is the second thing more than one service needs: the engine
//! parameter catalog and the parameter rules of the ONE AWS `rds` front door, which Amazon
//! RDS, Amazon DocumentDB and Amazon Neptune are three Smithy models of. The data is pure
//! — no modelled shape appears in it — so each of those crates keeps its own generated
//! shapes and state while the catalog and the rules over it live here once.
//!
//! [`query_error_envelope`] is the third: the `<RequestId>` element an `awsQuery`
//! `<ErrorResponse>` carries beside its `<Error>`, which no model declares and the
//! framework's error renderers do not write, for every service of that protocol whose
//! errors must carry the request id an SDK reports.
//!
//! [`query_timestamps`] is the fourth: the reading that lets an `@httpQuery` timestamp bind
//! in every form the wire names an instant in, for every service whose time-ranged reads
//! take their window in the query string — the generated schema carries no member-level
//! `@timestampFormat`, so without it the framework reads such a member as a `Z`-zoned
//! RFC-3339 string only and refuses both the epoch-seconds number a model may declare and
//! the numeric zone offset RFC 3339 equally admits. It also owns the answer a value that
//! names no instant earns, which is the framework's envelope rather than a modelled error:
//! such a value never becomes a modelled input.
//!
//! [`date_time_json`] is the BODY half of that same dropped trait: the `restJson1` codec
//! with a timestamp whose model declares `date-time` read and written as the RFC 3339
//! string every SDK bound to such a member parses, rather than as the protocol's default
//! epoch-seconds number.

pub mod rds_cluster_parameters;
/// The event source types the one `rds` front door accepts, and the sentence it refuses a
/// `SourceType` outside them with — one set for the three service views it serves.
pub mod rds_event_source_types;

/// The directory of which resources of the other services reference an ACM certificate,
/// written by the service that owns the referencing resource and read by ACM.
pub mod acm_certificate_associations;
/// The serving material of an ACM certificate — the leaf, its chain and the private key
/// that matches it — written by ACM when a caller imports key material into it and read by
/// the service that TERMINATES a connection presenting that certificate.
pub mod acm_certificate_key_material;
/// The inventory of which Active Directory directories the account holds, written by the
/// AWS Directory Service control plane — which creates and deletes them — and read by the
/// Directory Service DATA plane, which serves the objects inside a directory and creates
/// none, so it cannot tell a directory of this account from one that is not on its own.
pub mod ad_directories;
/// The SageMaker job directory the SageMaker control plane writes when it creates a job
/// of a reinforcement-fine-tuning category, and the AgenticRFT job runtime data plane
/// reads to resolve the job ARN its callers carry.
pub mod agentic_rft_jobs;
/// The inventory of which AgentCore Memory containers and code interpreters the account
/// holds, written by the Bedrock AgentCore CONTROL plane — which creates and deletes them —
/// and read by the AgentCore DATA plane, which creates neither and so cannot tell a
/// resource of this account from one that was never created.
pub mod agentcore_resources;
/// The registry of which resources are registered with Amazon Application Recovery
/// Controller for zonal shift, written by the service that OWNS the resource — ARC models
/// no operation that registers one — and read by ARC, which reports a managed resource it
/// cannot otherwise see.
pub mod arc_managed_resources;
pub mod arn;
/// Who judges the `@connections` endpoint a request was addressed through — whether its
/// invoke path names a stage an api of this emulator has DEPLOYED — installed by the
/// service that owns apis and stages and called by the service that serves `@connections`,
/// which cannot read that owner's store.
pub mod execute_api_connections_endpoint;
/// Serving an operation AUDITED: the ONE wrapper that records every call a service serves
/// as a management event, so no operation has to record its own and none is forgotten.
pub mod audited_operation;
/// How a service words the request the emulator's AUTHENTICATION layer refused: the
/// unknown-credential failure and the signature failure, each in that service's own code,
/// so neither is collapsed into the authorization denial `AccessDenied`.
pub mod authentication_refusal;
/// The ONE reading of the partial batch response an attempt that was handed a whole batch
/// answers — which records of the batch it did not process — for every service that polls a
/// source in batches and hands one over in a single call, so two such services cannot read
/// the same report differently.
pub mod batch_item_failures;
pub mod bedrock;
/// The ONE definition of the IAM principal this emulator's own long-term credential speaks
/// for: the user STS reports as the caller and IAM holds among the account's users, so the
/// two cannot disagree about who the caller is.
pub mod caller_principal;
/// The app-instance ADMIN directory the Amazon Chime SDK family shares: Chime SDK
/// Identity promotes an identity to an administrator and Chime SDK Messaging reads the
/// role, which it reserves two of its actions for.
pub mod chime_app_instance_admins;
pub mod chime_family;
/// The IDENTITY NAME directory the Amazon Chime SDK family shares: the name an app
/// instance's identity is registered under, keyed by its ARN, written by Chime SDK
/// Identity — which owns the identity — and read by Chime SDK Messaging, which reports
/// every participant as an ARN and a name.
pub mod chime_identity_names;
/// The TAG directory the Amazon Chime SDK family shares: the tags of a Chime resource,
/// keyed by its ARN, so whichever member serves one of the generic tagging routes reads
/// and writes the same tags.
pub mod chime_tags;
/// The ONE answer to which Amazon-Web-Services-published CloudFormation resource types no
/// resource provider stands behind: the class CloudFormation reports on a registry read of
/// such a type, and the one an API that acts through a provider's handlers has nothing to
/// call for — so the registry and that API cannot disagree about one type.
pub mod cfn_provisioning;
/// The ONE rendering and delivery of an API Gateway stage's ACCESS LOG — the `$context`
/// format turned into a record, and the write of that record into the CloudWatch Logs
/// destination the stage names — for both API Gateway planes, which configure and render it
/// the same way, so a REST stage and an HTTP api's stage cannot write one format differently.
pub mod api_gateway_access_logs;
/// The ONE rule for what a caller-supplied CloudWatch Logs log group NAME may be, for
/// every service whose API takes one and then writes into the group it names — so two
/// services asked whether CloudWatch Logs could hold a name cannot answer differently.
pub mod cloudwatch_logs;
/// The ONE gate on whether the account's Amazon Connect instance registry holds the
/// instance id a request names — and the ONE distinction between an id the account does not
/// hold (a request fault) and a registry that could not be read at all (a server fault) —
/// for every service whose operations require a usable Connect instance, so two of them
/// cannot disagree about a request they both refuse.
pub mod connect_instance_registry;
pub mod db_data_sources;
/// The ONE rendering of a DynamoDB STREAM RECORD as the envelope a consumer of that stream
/// is handed one in, for every service that reads a table's stream and passes its records on
/// — so two consumers of the same record are handed the same document.
pub mod dynamodb_stream_event;
/// The ONE reading of how a client ADDRESSES an emulated service's DATA plane — the
/// reserved path namespace, the first path segment, the host without its port, and whether
/// a host names a service at all — for every service that serves one, so the planes cannot
/// drift on which requests they claim.
pub mod data_plane_addressing;
/// WHICH service answers at an ADDRESS this emulator published for a resource's own data
/// plane — recorded by the service that OPENED the core listener and read by whatever has to
/// route an in-process client that was handed that address, which cannot read a service off
/// an address the way it can off an endpoint host.
pub mod published_data_planes;
/// The ONE rendering of a `restJson1` timestamp whose model declares the `date-time` form,
/// for every service of that class — which the generated member schemas drop, so without it
/// such a member goes on the wire as the protocol's default epoch-seconds NUMBER.
pub mod date_time_json;
/// The ONE free-text search over a stored document's text — the terms a query names, which
/// of them a text carries, the window of that text to report with the matches inside it, and
/// the match of one whole document — for every service whose API answers a caller's query
/// with the documents its own records hold, so two services asked the same question of the
/// same text cannot answer differently.
pub mod document_text_search;
/// The ONE reading of a caller-supplied EMAIL ADDRESS, for every service whose model
/// documents a member as one and targets it at a bare string — so two services asked
/// whether a value is an address cannot answer differently.
pub mod email_address;
/// The ONE renaming of the body member a JSON error carries its DETAIL TEXT in, for every
/// service whose wire form reports that text under a different member name from the one its
/// model declares — so a client reads one member whichever layer answered.
pub mod error_detail_member;
/// The TWO places a rendered JSON failure names its error SHAPE — the body's `__type` and
/// the `x-amzn-errortype` header — and the ONE restatement of that shape, for a service
/// whose wire names a different one there from the one the rendering layer wrote.
pub mod error_type_locator;
/// The ONE reading and matching of an EventBridge EVENT PATTERN, for every service
/// whose API accepts one — a rule's `EventPattern`, a pipe's `FilterCriteria` filter —
/// so two services handed the same pattern and event cannot disagree about whether it
/// matches, or about whether the pattern is one at all.
pub mod event_pattern;
/// The `vnd.amazon.eventstream` framing and routing seam a service with a
/// `@streaming` output member needs, which the scaffold does not generate.
pub mod event_stream;
pub mod feature_store_groups;
/// Where a SageMaker Feature Store feature group's records are replicated to in Amazon S3,
/// under which role, and when that destination is reported as blocked — the one layout and
/// the one permission question the control plane that CONFIGURES an offline store and the
/// data plane that WRITES it share.
pub mod feature_store_offline_store;
pub mod front_door;
/// The request-claim rule for the two emulated services whose SigV4 signing name is
/// `greengrass` — the 2017-06-07 and 2020-11-30 API versions of one product, which AWS
/// serves from one endpoint and tells apart by the `restJson1` route a request addresses.
pub mod greengrass;
pub mod iam_authorization;
/// The ONE reading of the published form of an IAM entity PATH, for every service that takes
/// one or builds one — the form IAM's own `pathType` publishes in prose and that its
/// `@pattern` cannot decide, since Smithy's `@pattern` is a find and that pattern's
/// alternation is anchored on one side only.
pub mod iam_entity_path;
/// The faults a RUNNING fault-injection experiment imposes on another service's API — who
/// they affect, which operations of which service, what share of the calls and for how long
/// — held once in the shared engine, so the service that runs the experiment and the service
/// whose API it affects read ONE set of them.
pub mod injected_api_faults;
/// The ONE reading of a data-plane connection whose ENCRYPTION is negotiated IN BAND: the
/// first byte that tells a TLS record from a request in the clear, and the replay of that
/// byte to whoever reads the stream next — for every service that serves one announced
/// address over both schemes.
pub mod in_band_tls;
/// The ONE reading of whether an in-process call to another emulated service reached that
/// service at all, rather than being refused by the front door for a request no mounted
/// service claims — so no performer of such a call maps the transport's own refusal as if
/// it were the addressed service's answer.
pub mod in_process_refusal;
/// The ONE SigV4 signature a service puts on an in-process call it makes to another
/// emulated service, computed with the settings the emulator's own verifier re-signs with.
pub mod in_process_sigv4;
/// The ONE construction of the in-process IAM client a service resolves a caller-supplied
/// IAM entity through, so that hop is configured once rather than per service.
pub mod iam_client;
/// The ONE grammar of an IAM JSON resource-based policy document, for every service whose
/// API takes one as a string and has to decide whether to accept it — so two services
/// handed the same document cannot disagree about whether it is a policy at all.
pub mod iam_policy;
/// The ONE reading of the FORMS a resource-based policy names a principal in — the principal
/// types the language defines and what a value of each may be — for every service that
/// decides whether a submitted policy document names principals at all.
pub mod iam_policy_principals;
/// The ONE rendering of an IAM resource-based policy document a service stores and reports
/// back — the whitespace between its JSON tokens dropped and a one-element string array
/// written as the string it names — so no two services report one policy's bytes
/// differently.
pub mod iam_policy_rendering;
/// The ONE resolution of an identity a resource-based policy grants to against this
/// account's IAM, for every service that resolves a policy document's principals before it
/// stores it.
pub mod iam_principal;
pub mod iam_role;
pub mod iam_trust_policy;
pub mod iot_commands;
pub mod iot_job_executions;
pub mod iot_jobs;
/// The ONE construction of the in-process Kinesis client a service DELIVERS a data record
/// through, so that hop — and the fact that it opens no socket — is configured once rather
/// than per service.
pub mod kinesis_client;
/// The ONE reading of an ISO 8601 DURATION a service is handed as a caller-supplied
/// string, for every service whose API takes a span that way — so two services asked how
/// long a duration is cannot answer differently.
pub mod iso8601_span;
/// The ONE minification of a JSON document a service takes as an opaque string member and
/// reports back on every read, so no two services report one document's bytes differently.
pub mod json_document;
/// The ONE reading of WHERE a JSON document a caller submitted stopped parsing — the
/// character or the unquoted word the parse stopped on, what was expected there, and the
/// line and column — for every service whose refusal carries its JSON parser's own
/// diagnostic.
pub mod json_parse_fault;
/// The ONE spelling of a request member as an RFC 6901 JSON pointer, built from the
/// dotted path the framework's constraint walk reports — for every service whose refusal
/// addresses a failing input that way rather than by the dotted path.
pub mod json_pointer;
/// The ONE naming of the CLASS of a JSON value a request carried, of the member a JSON key
/// names, and of the sentence refusing a value the modelled member it is bound to cannot be
/// read from — for every service whose request-deserialization layer answers such a value
/// in the protocol's own error class rather than as a constraint violation.
pub mod json_wire_values;
/// The ONE evaluation of a caller-supplied JSON path that has to name a SINGLE value,
/// for every service whose API takes such a path as a string — bounded to a reference
/// path, so what it costs is its segment count rather than the size of the document.
pub mod json_reference_path;
/// The ONE reading of a compact JWS (a JSON Web Token) a service is handed as a
/// caller-supplied string, for every service that has to decide whether a value is a token
/// at all before it reads a claim out of it.
pub mod jwt;
/// The SIGNING half of that reading, for every emulated service that IS an OIDC issuer:
/// the key pair it signs with, the public JWK its key set publishes, and the compact
/// serialization a signed token travels in — held once so that what one issuer signs and
/// what [`jwt::verify`] checks cannot diverge.
pub mod jws_signing;
/// The two documents such an issuer publishes about itself, where they are published and
/// how a request for one is recognised — so every issuer this emulator serves is
/// bootstrapped from the same paths.
pub mod openid_discovery;
/// The ONE bounded evaluation of a caller-written JSONata expression, for every service
/// whose API carries one — so the ceilings that confine an engine evaluating a caller's
/// text are stated, and met, in one place rather than per service.
pub mod jsonata;
/// The ONE bounded, session-local POSIX/bash shell session every service that has to
/// PERFORM caller-supplied shell text drives — a build's command lines, a container's
/// program, a notebook instance's lifecycle hook — so the catalog it performs, what it
/// fails closed on and the bounds that confine it are stated once rather than per service.
pub mod posix_shell_session;
/// The value of one ARITHMETIC expansion of that session (`$((…))`), evaluated over the
/// session's own variables and nothing else.
pub mod posix_shell_arithmetic;
/// The in-memory TREE that session reads and writes when the caller-supplied program is the
/// program of a MACHINE the emulator holds — its files, its sealed values and its devices,
/// all bounded, with no host path behind any of them.
pub mod posix_shell_volume;
pub mod lex;
pub mod management_events;
/// The ONE construction of the in-process EventBridge client a service publishing an event
/// drives, so no two services configure that hop differently.
pub mod eventbridge_client;
/// The ONE publication of a recorded management call onto the account's default
/// EventBridge bus — CloudTrail's `AWS API Call via CloudTrail` fan-out — so the channel is
/// stated once for every service whose calls that log records rather than per service.
pub mod api_call_events;
/// The ONE delivery of a DATA event — a call on the CONTENTS of a resource — into the
/// bucket of every trail whose event selectors admit it, with the match against those
/// selectors, so both are stated once rather than per service.
pub mod data_events;
/// The ONE construction of the in-process Amazon S3 client a service writing an object
/// drives, so no two services configure that hop differently.
pub mod s3_client;
/// The ONE construction of the in-process Amazon SNS client a service publishing a
/// notification drives, so no two services configure that hop differently.
pub mod sns_client;
/// The ONE construction of the in-process AWS Organizations client a service acting on an
/// organization drives, so no two services configure that hop differently.
pub mod organizations_client;
/// The ONE construction of the in-process AWS Systems Manager client a service running a
/// document or resolving a parameter drives, so no two services configure that hop
/// differently.
pub mod ssm_client;
/// The ONE read of an Amazon S3 object a service was pointed at, and the ONE reading of
/// what the object store's answer about it established, over that same hop.
pub mod s3_object_read;
/// The ONE delivery of a recorded management call into the bucket of each trail that logs
/// it — the gzipped log file an audit pipeline reads — so the layout and the framing are
/// stated once rather than per service.
pub mod trail_log_files;
/// The ONE reading of the JSON Schema document a gateway model carries, and the two
/// documents a gateway control plane generates from it: the sample payload one answers a
/// template request with, and the mapping skeleton the other does.
pub mod model_mapping_template;
pub mod network;
/// The framing of a response that carries no content: a success whose modelled output has
/// no member answers 204 with no body, not the empty JSON object a serializer renders.
pub mod no_content_responses;
/// The ONE way a service tells a continuation token it ISSUED from one a caller built,
/// for every paginated read whose `NextToken` carries the position inside it.
pub mod page_tokens;
/// The ONE reading of whether a resource-based policy document grants PUBLIC access — a
/// grant to a principal nobody named — for every service that refuses such a document or
/// reports a policy's reach.
pub mod public_resource_policy;
/// A floating-point member of a response rendered the way AWS renders one — with a
/// fractional part — which the framework's XML writer drops for a whole value.
pub mod query_decimal_floats;
pub mod query_error_envelope;
/// A refusal whose clause QUOTES the value the request carried, where the framework's own
/// walk elides a long one into a prefix AWS never emits.
pub mod quoted_refusals;
/// An `@httpQuery`-bound timestamp read in every form the wire names an instant in, which
/// the generated schema narrows to one and the framework therefore refuses.
pub mod query_timestamps;
/// The ONE rendering of the envelope a service states around a refusal ANOTHER service
/// answered it with: the called service, the status it answered, that call's request id and
/// how many attempts it took.
pub mod relayed_refusal;
/// The media type a service announces on a response, where the recorded wire differs from
/// the one its protocol trait implies.
pub mod response_media_type;
/// The ONE plumbing for a service that answers a framework-rendered refusal in an envelope
/// only the request and the whole rendered response can decide.
pub mod restated_refusal;
pub mod rfc3339;
pub mod rpc_target;
/// The ONE reading of what an Amazon S3 BUCKET NAME has to be, for every service that
/// takes one as a caller-supplied string and points a resource of its own at it.
pub mod s3_bucket_name;
/// The account's EMAIL IDENTITY registry, shared by the two SES surfaces: the set of
/// identities one account holds and the verification state of each, written by
/// whichever surface records or deletes one and read by both.
pub mod ses_identities;
/// The account's CONFIGURATION SET registry, shared by the two SES surfaces: the set of
/// configuration sets one account holds and the options of each that both APIs publish,
/// written by whichever surface records, configures or deletes one and read by both.
pub mod ses_configuration_sets;
/// The account's EMAIL TEMPLATE registry, shared by the two SES surfaces: the set of email
/// templates one account holds and the content of each, written by whichever surface
/// records, replaces or deletes one and read by both — and the continuation token of the
/// one template listing those two APIs page.
pub mod ses_templates;
/// Which member paths a boundary refusal states no value for, read from a service's own
/// Smithy model: the members it marks `@sensitive`.
pub mod sensitive_members;
/// The cross-service calls one emulated service is making AS ITS OWN SERVICE PRINCIPAL, on
/// behalf of one of its resources — written by the calling service for as long as its call
/// is in flight, read by the service that has to authorize that call and cannot read the
/// caller's store.
pub mod service_principal_calls;
pub mod shared_path;
/// How AWS Signer states a refusal of an input member a request supplied nothing for, for
/// the two service crates that are two views of that one endpoint.
pub mod signer_input_refusals;
/// The ONE home for the half of a constraint refusal the framework's walk cannot state —
/// the COLLECTION a clause about a collection member is about — served as the
/// per-operation wrapper that is the only layer with the request it has to be read from.
pub mod submitted_collections;
/// The ONE list of SQL functions caller SQL may call, for every data plane that runs it on
/// the in-process SQLite engine.
pub mod sqlite_confinement;
/// The ONE token bucket an emulated service meters a caller-driven request rate with, for
/// every service whose model publishes a rate-and-burst pair.
pub mod token_bucket;
pub mod validation;
pub mod validation_message;
/// The web identity tokens this emulator itself issued: the registry a redeemer of one
/// reads, written by the service that issued it.
pub mod web_identity_tokens;
/// The ONE reading of which media type a static file's extension names, for every data
/// plane that serves deployed files.
pub mod web_media_type;
/// The ONE construction of the in-process X-Ray client a service REPORTS its own segments
/// through, and the one submission of the documents it reports.
pub mod xray_segments;
/// The ONE reading of an AWS X-Ray trace id and tracing header, for every service that
/// takes one as a caller-supplied string — so X-Ray and a service that merely carries a
/// trace header cannot disagree about what counts as one.
pub mod xray_trace;
/// The ONE bound on how deeply a caller-supplied YAML document nests, taken before that
/// document is handed to a parser — so how deeply a caller's text may nest is decided in
/// one place rather than per service.
pub mod yaml_nesting;
pub use validation_message::aws_validation_refusal;

use aws_smithy_runtime_api::http::Request as RuntimeApiRequest;
use aws_smithy_types::body::SdkBody;
use emulator_server::{resolve_route_key, ROOT_PATH_SEGMENT};
use smithy_server_runtime::ErasedServiceDefinition;

/// A stable 64-bit mixing of `parts`, rendered as 16 hexadecimal digits.
///
/// For an id a service DERIVES from what it identifies and then puts ON THE
/// WIRE — X-Ray's inferred segment id, say. Two properties matter and neither is
/// free:
///
/// * the same input mixes to the same id on every READ, so a caller that reads a
///   derived resource twice sees one id; and
/// * the same input mixes to the same id in every BUILD, which `std`'s
///   `DefaultHasher` does not promise (`std` reserves the right to change its
///   algorithm), so an id taken from it is reproducible only within one
///   toolchain.
///
/// The mixing is SHA-256 from the maintained `sha2` crate — nothing is
/// hand-rolled here — over the parts, each followed by a NUL so that two
/// different part lists whose concatenation coincides still mix differently; the
/// id is its first 8 bytes. Truncated like that it is an OPAQUE id, not an
/// unguessable one: nothing may authenticate on it.
///
/// Several service crates still carry their own hand-written mixing function
/// (`grep -rln 'FNV' services/*/src/state.rs`); they are to be migrated onto
/// this one, and nothing new should write another.
#[must_use]
pub fn stable_hex_id(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update(b"\0");
    }
    digest.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `bytes` as the text of a version-4 UUID: lowercase hexadecimal in the hyphenated
/// `8-4-4-4-12` groups, carrying the version and variant bits a UUID built from random
/// bytes has.
///
/// For an identifier a service MINTS in that form, out of 16 opaque bytes the service
/// itself produced. The bytes stay the caller's: this RENDERS them rather than
/// generating them, so a service keeps whatever unpredictability its own state module
/// documents and takes only the layout from here.
///
/// Both halves of that layout belong to the maintained `uuid` crate — one call writes
/// the version and variant bits, the other the text form — so no service has to spell
/// out the bit masks or the group boundaries, which is exactly where a second copy
/// drifts into producing something that is no longer a UUID.
///
/// Service crates still carrying their own copy of those masks and that `format!` are
/// to be migrated onto this one, and nothing new should write another.
#[must_use]
pub fn uuid_from_random_bytes(bytes: [u8; 16]) -> String {
    ::uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .hyphenated()
        .to_string()
}

/// The fixed first path segment of every `rpcv2Cbor` request
/// (`POST /service/{Service}/operation/{Operation}`), as
/// `emulator-core-build-support` publishes it in `ROUTE_PATH_SEGMENTS` for a service
/// that speaks that protocol.
pub const RPCV2_CBOR_PATH_SEGMENT: &str = "service";

/// Whether `service` claims `parts`: its signing name matches the request's resolved
/// routing key AND the request addresses this service's own modelled wire surface.
///
/// * `signing_name` — the service's SigV4 signing name
///   (`definition().identity().signing_name()`).
/// * `path_segments` — the first URI path segments the model declares, as the
///   generated `crate::ROUTE_PATH_SEGMENTS`.
/// * `rpc_service_shape_id` — `Some(shape id)` when the model declares an RPC
///   protocol (`awsJson1_0`, `awsJson1_1`, `rpcv2Cbor`), whose wire form names the
///   service by the shape's LOCAL name; `None` for a purely REST service, which is
///   then discriminated by its path space exactly as the framework does.
///
/// An RPC service claims ONLY requests that name it AND arrive on a path its protocol
/// uses: its path space is the protocol's own (`/`, or `/service/...`), which every RPC
/// peer sharing the signing name also posts to, so a path test alone cannot tell them
/// apart — and a request that names one service while addressing another's REST path
/// (`POST /CancelChangeSet` with an `X-Amz-Target`) contradicts itself, so it must reach
/// NOBODY rather than two claimants. A model that declares an RPC protocol AND binds
/// some operations to URIs (Glue and Transfer each do) keeps both claims: the RPC form,
/// or one of its own literal path segments.
pub fn claims_request(
    parts: &http::request::Parts,
    signing_name: &str,
    path_segments: &[&str],
    rpc_service_shape_id: Option<&str>,
) -> bool {
    claims_request_naming(
        parts,
        signing_name,
        path_segments,
        rpc_service_shape_id,
        rpc_target::names_service,
    )
}

/// [`claims_request`] for an RPC service whose signing name is shared with ANOTHER API
/// VERSION of itself: the service an `X-Amz-Target` names is read with the API-version
/// suffix compared rather than dropped ([`rpc_target::names_service_version`]).
///
/// One endpoint serving several API versions of a service is normal at AWS — both versions
/// carry the same `endpointPrefix` and the same `sigv4Name`, and the version is named only
/// by the target's service-shape prefix — so the signing name tells the two apart no better
/// than it tells either from the other. [`claims_request`] still leaves both claiming every
/// request there, because the comparison it inherits from the framework's routing key drops
/// the very suffix that differs; this is the same rule with that suffix kept, so each
/// version claims exactly its own traffic.
///
/// `service_shape_id` is REQUIRED (not `Option`): a shared signing name with no RPC wire
/// form to read the version from has no discriminator at all, and such a pair must be
/// separated by its path space through [`claims_request`] instead.
pub fn claims_versioned_rpc_request(
    parts: &http::request::Parts,
    signing_name: &str,
    path_segments: &[&str],
    service_shape_id: &str,
) -> bool {
    claims_request_naming(
        parts,
        signing_name,
        path_segments,
        Some(service_shape_id),
        rpc_target::names_service_version,
    )
}

/// [`claims_request`]'s body, with the comparison that decides whether an RPC wire form
/// NAMES this service handed in, so the two claim rules cannot drift into two different
/// readings of a path or a routing key.
fn claims_request_naming(
    parts: &http::request::Parts,
    signing_name: &str,
    path_segments: &[&str],
    rpc_service_shape_id: Option<&str>,
    names_service: fn(&str, &str) -> bool,
) -> bool {
    if !routes_to(parts, signing_name) {
        return false;
    }
    match rpc_service_shape_id {
        None => claims_modeled_path(parts, path_segments),
        Some(shape_id) if is_rpc_addressed(parts) => {
            rpc_names_this_service(parts, shape_local_name(shape_id), names_service)
                && (claims_rpc_path(parts) || claims_literal_path(parts, path_segments))
        }
        // Not addressed in an RPC form at all: only a URI this model binds itself.
        Some(_) => claims_literal_path(parts, path_segments),
    }
}

/// The borrowed request `parts` as the runtime-API request a `ServerProtocol` reads,
/// with an empty body. `None` when the URI or method is one the runtime API refuses.
///
/// This mirrors the router's own `runtime_api_request_from_parts`
/// (`vendor/smithy-server-runtime/src/multi_service_router.rs`), which is private to that
/// agent-immutable crate and so cannot be called here; keep the two in step. It differs
/// only in carrying no body (no route lookup reads one) and in answering `None` where the
/// router, which has already accepted the request, may expect a valid URI and method.
fn runtime_request_from(parts: &http::request::Parts) -> Option<RuntimeApiRequest> {
    let mut request = RuntimeApiRequest::new(SdkBody::empty());
    request.set_uri(parts.uri.to_string().as_str()).ok()?;
    request.set_method(parts.method.as_str()).ok()?;
    for (name, value) in parts.headers.iter() {
        if let Ok(value) = value.to_str() {
            // `append`, not `insert`: `HeaderMap::iter` yields one entry per value, so a
            // repeated or multi-valued header keeps every one of them, as the router's own
            // conversion does.
            request
                .headers_mut()
                .append(name.as_str().to_string(), value.to_string());
        }
    }
    Some(request)
}

/// Whether `parts` is addressed in an RPC form: `awsJson1_0` / `awsJson1_1` name their
/// target service in `X-Amz-Target: <Service>.<Operation>`, and `rpcv2Cbor` in the URI
/// `/service/<Service>/operation/<Operation>`.
fn is_rpc_addressed(parts: &http::request::Parts) -> bool {
    parts.headers.contains_key(rpc_target::TARGET_HEADER)
        || rpcv2_cbor_service_named(parts.uri.path()).is_some()
}

/// Whether the service an RPC-addressed request names is the one whose shape-local name
/// is `declared`.
///
/// This is why an RPC peer does not also claim a REST peer's path: a request carrying an
/// `X-Amz-Target` that names an RPC service is claimed by that service only on a path
/// its protocol uses, so the path's own owner serves it alone.
///
/// The `X-Amz-Target` side is read through [`rpc_target::names_service`], this crate's
/// one statement of the target rule: the service is the last dot-separated segment of the
/// target's PREFIX, compared the way the framework compares a routing key (trailing
/// `_<digits>` API-version suffix dropped, case-insensitive), so a client sending either
/// the versioned or the unversioned form reaches the service either way.
///
/// It is read there rather than through the framework's own
/// `emulator_server::service_from_target` because that reader takes the segment before
/// the FIRST dot, which is the service only while the target prefix is a single
/// identifier: a service whose published prefix is its fully qualified Coral name or its
/// absolute shape id is named by a namespace label (`com`) under that reading, so it
/// claimed none of its own traffic. The `rpcv2Cbor` side has no framework reader: its
/// `{Service}` label is compared case-insensitively.
///
/// A READABLE `X-Amz-Target` decides on its own: a request that names its service out of
/// band in the `awsJson` form is answered by the service it names, and the `rpcv2Cbor` URI
/// is consulted only for a request that carries no such header (one that does and names
/// another service contradicts itself, so it reaches nobody). The URI is still the reader
/// for a header whose bytes are not text at all, which names no service.
///
/// `names_service` is the comparison the CALLER chose for the `X-Amz-Target` half —
/// [`rpc_target::names_service`] for peers whose shape names differ by more than an
/// API-version suffix, [`rpc_target::names_service_version`] for two API versions of one
/// service, which that suffix is the only difference between.
fn rpc_names_this_service(
    parts: &http::request::Parts,
    declared: &str,
    names_service: fn(&str, &str) -> bool,
) -> bool {
    if let Some(target) = parts
        .headers
        .get(rpc_target::TARGET_HEADER)
        .and_then(|target| target.to_str().ok())
    {
        return names_service(target, declared);
    }
    rpcv2_cbor_service_named(parts.uri.path())
        .is_some_and(|named| named.eq_ignore_ascii_case(declared))
}

/// Whether `parts` arrives on a path an RPC protocol uses: the ROOT path every
/// `awsJson` operation posts to, or the `/service/<Service>/operation/<Operation>` an
/// `rpcv2Cbor` one does.
fn claims_rpc_path(parts: &http::request::Parts) -> bool {
    let path = parts.uri.path();
    first_path_segment(path).is_none() || rpcv2_cbor_service_named(path).is_some()
}

/// The modeled OPERATION `definition` resolves for the request, or `None` when its model
/// binds no route for the request's method and URI.
///
/// Both halves of the framework's own lookup, in the order and with the data the router
/// uses (`MultiServiceRouter::dispatch`): the service's wire protocol reads the routing
/// key, and the model-derived routing table must actually hold an operation under it. The
/// second half is what makes this a sound claim test for EVERY protocol and not only the
/// REST family: `awsJson`'s and `rpcv2Cbor`'s key readers merely PARSE
/// `X-Amz-Target: <Service>.<Operation>` or `/service/<S>/operation/<O>` and succeed for
/// any well-formed one whatever service owns it, so the key alone would claim a peer's
/// traffic; the table lookup fails for an operation this model does not declare.
///
/// The request's parts are re-presented to the protocol as a BODILESS runtime-API
/// request: a REST, `awsJson` or `rpcv2Cbor` routing key is read from the method, the URI
/// and the headers, all of which `Service::can_handle` is handed, so nothing the decision
/// rests on is missing. The `query` family reads its key from the BODY, which
/// `can_handle` does not receive, so an `awsQuery` / `ec2Query` service uses
/// [`claims_request`] instead.
///
/// Returning the operation NAME, rather than only "yes, a route": for the residue the
/// path half of a claim cannot split — two peers sharing a signing name whose models bind
/// the IDENTICAL method and URI, where no signal in the request tells them apart and the
/// route must therefore be served by exactly one of them — the one that cedes can name the
/// ceded OPERATIONS rather than restating their method and URI templates beside the model.
/// The route is read off the model by the same protocol that will route the request, so a
/// ceded route cannot drift from the one the model binds.
pub fn modeled_operation(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
) -> Option<String> {
    let request = runtime_request_from(parts)?;
    let key = definition.protocol().routing_key(&request).ok()?;
    definition.routing_table().get(key).map(|_| key.to_string())
}

/// Whether the routing key the framework resolves for `parts` is `signing_name`.
///
/// This is `emulator_server::can_handle_based_on_route_key`'s rule applied over
/// `Parts`: that helper takes a whole buffered `http::Request`, which the
/// `Service::can_handle` seam does not receive. Comparison is case-insensitive on a
/// trimmed name, as there.
///
/// The one-key case of [`routes_to_any`], which it delegates to so the two cannot
/// disagree about what "the key names this service" means.
pub fn routes_to(parts: &http::request::Parts, signing_name: &str) -> bool {
    routes_to_any(parts, &[signing_name])
}

/// Whether the routing key the framework resolves for `parts` is one of `route_keys`.
///
/// [`routes_to`] over the several keys ONE endpoint answers on. A service's SigV4
/// signing name is not always the name its endpoint is addressed by: the framework
/// resolves the routing key from the credential scope first (the signing name), and
/// falls back to the `Host`/`:authority` label (the `endpointPrefix`) for a request
/// that carries no scope. Both name the same endpoint, so both are its keys.
pub fn routes_to_any(parts: &http::request::Parts, route_keys: &[&str]) -> bool {
    match resolve_route_key(parts) {
        Some(key) => route_keys
            .iter()
            .any(|route_key| key.eq_ignore_ascii_case(route_key.trim())),
        None => false,
    }
}

/// The access key id a request presents, wherever SigV4 put it.
///
/// SigV4 has two schemes. "Authenticating Requests: Using Authorization Header" carries
/// the credential scope in the `Authorization` header, which is what
/// `::emulator_server::extract_akid` reads; "Authenticating Requests: Using Query
/// Parameters", the one a presigned URL uses, carries the same scope in the
/// `X-Amz-Credential` query parameter instead. The framework VERIFIES both, so a service
/// resolving who a request speaks for has to read both — a presigned call is made by the
/// credential that signed the URL, not by nobody.
///
/// Answers `extract_akid`'s own sentinel for a request that presents no credential at all,
/// so an unsigned request reads exactly as it does through that function alone. One reader
/// for the tree: `extract_akid` does not export its sentinel and the framework's own
/// presigned-field parsing is private, so without this every service that serves a
/// presigned call grows its own copy.
pub fn request_access_key_id(parts: &http::request::Parts) -> String {
    let from_header = ::emulator_server::extract_akid(&parts.headers);
    if from_header != UNSIGNED_ACCESS_KEY_ID {
        return from_header;
    }
    presigned_credential_key_id(parts.uri.query().unwrap_or("")).unwrap_or(from_header)
}

/// What `::emulator_server::extract_akid` answers for a request carrying no SigV4
/// credential scope. Stated here because that function does not export it.
const UNSIGNED_ACCESS_KEY_ID: &str = "default";

/// The key id of a presigned request's `X-Amz-Credential` scope
/// (`<key id>/<date>/<region>/<name>/aws4_request`), whose `/` separators a client
/// percent-encodes.
///
/// The key id itself carries neither separator — an access key id is `^[\w]*$` in every
/// AWS model that constrains one — so the scope's first field ends at whichever of them
/// comes first.
fn presigned_credential_key_id(query: &str) -> Option<String> {
    let credential = query_parameter(query, "X-Amz-Credential")?;
    let key_id = credential.split(['/', '%']).next().unwrap_or_default();
    (!key_id.is_empty()).then(|| key_id.to_string())
}

/// The value of the query parameter `name`, read off a URI's raw query string.
///
/// Parameter NAMES in a SigV4 query are spelled literally, so the name is compared as
/// written; the value is returned as written, which is what its readers here need — a
/// credential scope ([`request_access_key_id`]) and a query-protocol `Action` are drawn
/// from character sets a client does not escape.
pub fn query_parameter<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query
        .split('&')
        .filter_map(|item| item.split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// The claim rule for a service whose signing name is SHARED and whose peers' modelled
/// path spaces OVERLAP: the request's resolved routing key names this endpoint, and this
/// service's own model binds a route for the request.
///
/// * `route_keys` — the keys this endpoint answers on: its signing name
///   (`definition().identity().signing_name()`) and its `endpointPrefix`, which the model
///   states and a host-addressed request resolves to.
/// * `definition` — this service's model-derived half (`Service::definition()`), whose
///   protocol and routing table are the model's own statement of which requests this
///   service serves. The route half is [`addresses_modeled_route`], so route matching
///   stays in the one place that owns it.
///
/// This is [`claims_request`]'s rule with its path conjunct sharpened from "a first path
/// segment my model declares" to "a whole route my model binds", which is how AWS itself
/// dispatches a REST request once the endpoint host has chosen the service. Peers whose
/// first path segments collide are therefore still separated, as long as their models
/// bind no identical method + URI pattern; where two peers DO bind the same route, the
/// request is ambiguous on this rule alone and needs a further discriminator read off
/// the request (see [`addresses_runtime_bidirectional_stream`] and [`shared_path`] for
/// that shape).
///
/// A request this service's model does not bind is NOT claimed, even when no peer binds
/// it either: a path no emulated model publishes is the router's `404 UnknownService`,
/// which is honest, rather than one peer answering for an endpoint it does not serve.
pub fn claims_modeled_route(
    parts: &http::request::Parts,
    route_keys: &[&str],
    definition: &dyn ErasedServiceDefinition,
) -> bool {
    routes_to_any(parts, route_keys) && addresses_modeled_route(parts, definition)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first
/// URI path segments a service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, keeping both of its
/// edge cases: an EMPTY list means the model published no usable discriminator and is
/// "no constraint", and [`ROOT_PATH_SEGMENT`] is how a service that must answer
/// `POST /` claims the root path without claiming every path. Matching is EXACT
/// segment equality, deliberately never a prefix test.
pub fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&ROOT_PATH_SEGMENT),
    }
}

/// Whether the service whose generated `ServiceDefinition` is `definition` binds a
/// modelled route for the request `parts` came from.
///
/// The LAST discriminator available to two peers under one signing name whose modelled
/// path spaces overlap and whose URIs do not differ at any single segment
/// ([`nth_path_segment`] is the cheaper reader for that case). It names nothing about
/// either service: the question it asks is exactly the one the router asks AFTER a
/// service claims a request — the service's own wire protocol resolves the request's
/// routing key (for a REST protocol, from the method and the URI patterns its model
/// binds; for `awsJson`, from `X-Amz-Target`; for `rpcv2Cbor`, from the URI), and the
/// model-derived routing table answers whether that key names one of this service's
/// operations. So a peer claims a path inside the space it shares only for the routes its
/// OWN model binds, and the framework's route matcher stays the single owner of route
/// matching.
///
/// `Service::can_handle` is handed the request's parts, while a protocol reads a whole
/// runtime-API request, so the three things a routing key is read from — the method, the
/// URI and the headers — are re-presented to it on an EMPTY body, through
/// [`runtime_request_from`], the ONE copy in this crate of the router's own private
/// rebuild: every claim phrasing here goes through it, so the crate presents one request
/// to one `routing_key`. No protocol that keys on the body is affected: `awsQuery` and
/// `ec2Query` route on the `Action` form field, publish no `smithy.api#http` bindings at
/// all, and are therefore separated by the service their request NAMES rather than by a
/// route (see [`claims_request`]).
///
/// `false` when the request cannot be rebuilt (a URI or method the runtime API rejects),
/// when the protocol resolves no routing key, and when the key it resolves names no
/// operation this service wires — the boolean form of [`modeled_operation`], which it
/// delegates to so that "does my model bind this route" is asked in exactly one place.
pub fn addresses_modeled_route(
    parts: &http::request::Parts,
    definition: &dyn ErasedServiceDefinition,
) -> bool {
    modeled_operation(parts, definition).is_some()
}

/// [`claims_modeled_path`] over the LITERAL segments only: the two protocol
/// sentinels (`/` for an RPC service's root post, `service` for an `rpcv2Cbor` one)
/// are dropped, and an empty list claims nothing.
///
/// Used for a service that declares an RPC protocol, where those sentinels are the
/// path space every peer sharing the signing name posts to. Its RPC traffic is
/// claimed by name instead, so here only a URI its own model binds counts.
fn claims_literal_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.iter().any(|declared| {
            *declared == segment
                && *declared != ROOT_PATH_SEGMENT
                && *declared != RPCV2_CBOR_PATH_SEGMENT
        }),
        None => false,
    }
}

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// The PATH of a request URI: no scheme, no authority, no query string and no fragment.
///
/// The framework hands `Service::can_handle` an `http::request::Parts`, whose `uri.path()`
/// is already just the path — but the two seams that take a whole runtime-API request
/// (`Service::override_operation`, `Service::override_protocol`) see the URI as a `&str`
/// that may be absolute (`https://host/path?query`), which is the form the AWS SDK sends.
/// A service deciding anything from the path of such a request needs the path alone, and
/// several crates had each grown a private copy of this reader; this is the shared one, so
/// [`nth_path_segment`] and [`first_path_segment`] can be applied to a whole URI as well
/// as to a `Parts` path.
///
/// A URI with no authority is already a path and is returned as it is. A URI whose
/// authority is followed by nothing has the root path, which is what a request to the
/// authority addresses.
pub fn uri_path(uri: &str) -> &str {
    let after_authority = match uri.find("://") {
        None => uri,
        Some(scheme_end) => {
            let authority = &uri[scheme_end + 3..];
            match authority.find('/') {
                Some(path_start) => &authority[path_start..],
                None => "/",
            }
        }
    };
    after_authority
        .split(['?', '#'])
        .next()
        .unwrap_or(after_authority)
}

/// Whether `req` addresses the route ONE modelled operation's `@http` trait binds: that
/// trait's METHOD, and its URI pattern with the `@httpLabel` placeholders filled.
///
/// `input_schema` is that operation's own `Operation::INPUT_SCHEMA`, which carries the
/// `@http` binding, so the METHOD and the PATTERN are read off the model rather than
/// copied into a literal per service that cannot be kept in step with it.
///
/// This answers the question the framework's own route lookup cannot be asked: which
/// operation a request ADDRESSES when no route matched it. The REST router rejects a
/// candidate route whose `@required` header/query bindings are absent from the request
/// (`route_lookup`, `vendor/smithy-schema-server/src/rest_xml.rs`), which is how an
/// S3-class model tells several operations apart on ONE method + URI template by the
/// shape of the request; for a model that binds a method + URI template to exactly one
/// operation, the same rule only withholds the route of a request that left a required
/// parameter out, and the service then has nothing to answer it from. A service with such
/// a route asks this of each candidate operation from its `Service::override_operation`
/// seam, so the operation the model binds serves the request and refuses the absent member
/// itself. [`addresses_modeled_route`] is the other half of that pair and stays the
/// default: it asks the framework whether a route matched at all.
///
/// # Services still carrying their own copy
///
/// This reader postdates the private copies in `rum`, `artifact` and
/// `directoryservicedata` (`grep -rln 'fn path_matches' services/*/src/service.rs`), which
/// are to be MIGRATED onto it; they are left in place only because this change does not
/// own those services' issues and each is covered by its own suite. Nothing new should
/// inline the rule again — call this crate.
pub fn addresses_modeled_http_route(
    req: &RuntimeApiRequest,
    input_schema: &aws_smithy_schema::Schema,
) -> bool {
    input_schema.http().is_some_and(|binding| {
        binding.method().eq_ignore_ascii_case(req.method())
            && path_matches_http_uri(binding.uri(), uri_path(req.uri()))
    })
}

/// Whether `path` is the `@http` URI `pattern` with its `@httpLabel` placeholders filled:
/// the same segments in the same order, a literal segment matching itself, a `{label}`
/// matching ONE non-empty segment and a greedy `{label+}` taking every segment that is
/// left (of which there must be at least one).
///
/// Only the pattern's PATH is matched. A query literal a pattern pins (`/{Bucket}?acl`)
/// SELECTS between routes that share a path, and selecting is what the framework's own
/// lookup does whenever it can; a caller here has already been told that no route matched.
///
/// Segments are read the way the framework's matcher reads them — leading and trailing `/`
/// ignored, nothing else skipped, an empty remainder being no segments at all — so this and
/// a service's generated routing table cannot disagree about which requests the table
/// already resolves.
pub fn path_matches_http_uri(pattern: &str, path: &str) -> bool {
    let expected = path_segments(pattern.split('?').next().unwrap_or(pattern));
    let actual = path_segments(path);
    for (index, segment) in expected.iter().enumerate() {
        let label = segment.starts_with('{') && segment.ends_with('}');
        let greedy = label && segment.ends_with("+}");
        match actual.get(index) {
            // The pattern has a segment the request does not.
            None => return false,
            // A greedy label takes every segment that is left.
            Some(carried) if greedy => return !carried.is_empty(),
            Some(carried) if label => {
                if carried.is_empty() {
                    return false;
                }
            }
            Some(carried) => {
                if carried != segment {
                    return false;
                }
            }
        }
    }
    // No greedy label took the remainder, so a longer request path is another route.
    expected.len() == actual.len()
}

/// The `/`-separated segments of a URI path, as the framework's route matcher splits them:
/// leading and trailing `/` ignored, and a path with nothing else in it having NO segments
/// (so the root path and the empty pattern are the same zero segments).
fn path_segments(path: &str) -> Vec<&str> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    }
}

/// The `index`-th (0-based) `/`-separated segment of a URI path, or `None` when the path
/// has no such segment.
///
/// For two peers under one signing name whose modelled URIs share their first path
/// segment, so that [`claims_modeled_path`] cannot separate them: the segment their URIs
/// differ at decides instead. Leading `/` is ignored and empty segments are NOT skipped,
/// so the index is the position in the URI as written.
///
/// That last part is deliberately UNLIKE [`first_path_segment`], which maps an empty
/// first segment to `None` so that a ROOT path can be told from a named one. Here an
/// empty segment is reported as `Some("")`: a caller is comparing a deeper segment
/// against a literal its model binds, which is never empty, so an empty segment must
/// compare unequal rather than disappear.
pub fn nth_path_segment(path: &str, index: usize) -> Option<&str> {
    path.trim_start_matches('/').split('/').nth(index)
}

/// One `@httpLabel` of a URI path as the codec that deserializes the request decodes it:
/// percent escapes resolved, and `+` kept VERBATIM.
///
/// [`smithy_server_runtime::percent_decode`] is a QUERY decoder — it folds `+` to a space,
/// which is the `application/x-www-form-urlencoded` rule — while a path label carries PATH
/// semantics, where `+` is a literal character of the value (the schema server decodes one
/// with its own `percent_decode_path`, which it does not export). A `+` can never occur
/// inside an escape, so decoding BETWEEN the `+` characters and putting them back keeps the
/// literal while leaving every escape to the shared helper: a `%2B` still decodes to `+`,
/// because it carries no literal `+` to split on.
///
/// This is for a service that reads a label off the REQUEST LINE — the
/// `Service::override_operation` / `override_protocol` seams, which see the URI before any
/// input is deserialized, and so cannot ask the deserializer what the label said. It sits
/// beside [`uri_path`] and [`nth_path_segment`], the readers that get it that label.
///
/// One copy of the rule predates this helper: `fn percent_decode` in
/// `service-com-amazonaws-inspectorscan-inspectorscan/src/ops/scan_sbom.rs`, which applies
/// it to a purl component for the same reason (`+` is literal in a package version). It is
/// to be MIGRATED onto this, in the change that owns that service; nothing new should
/// inline the rule again.
#[must_use]
pub fn percent_decode_path_label(label: &str) -> String {
    // Nothing to resolve and no `+` to preserve: the label is already its own value.
    if !label.contains('%') && !label.contains('+') {
        return label.to_string();
    }
    label
        .split('+')
        .map(smithy_server_runtime::percent_decode)
        .collect::<Vec<String>>()
        .join("+")
}

/// The path of a request URI, without its query string and without the authority an
/// absolute-form URI carries.
///
/// A server usually sees the origin form (`/model/m-1/invoke?x=1`), but an SDK that was
/// given an endpoint can send the absolute form (`https://host/model/m-1/invoke`), and
/// `aws_smithy_runtime_api::http::Request::uri` hands over whichever arrived. A service
/// that routes on the URI — reading the tail of a modelled path in `override_operation`,
/// say — needs the path alone, so it reads it through this one function rather than
/// splitting the URI itself.
///
/// # Services still carrying their own copy
///
/// The same extraction is inlined in several service crates that needed it before there
/// was anywhere shared to put it. The list is NON-EXHAUSTIVE — `grep -rn 'find("://")'
/// services/*/src` is the authority — and includes
/// `services/service-com-amazonaws-transcribestreaming-transcribe/src/service.rs`,
/// `services/service-com-amazonaws-applicationsignals-applicationsignals/src/service.rs`,
/// `services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/src/ops/invoke_endpoint_with_bidirectional_stream.rs`,
/// `services/service-com-amazonaws-artifact-artifact/src/service.rs`,
/// `services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/service.rs`,
/// `services/service-com-amazonaws-lightsail-lightsail-20161128/src/service.rs`,
/// `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/service.rs`,
/// `services/service-com-amazonaws-sso-swbportalservice/src/service.rs` and
/// `services/service-com-amazonaws-workspacesweb-awserminecontrolplaneservice/src/service.rs`
/// (which returns an owned `Option<String>`). Those copies are to be MIGRATED onto this
/// function; they are left in place because this change does not own those services'
/// issues and each one's routing is judged by its own suite. Nothing new should inline
/// the split again — call this.
pub fn request_path(uri: &str) -> &str {
    let path = uri.split('?').next().unwrap_or_default();
    match path.find("://") {
        Some(scheme_end) => match path[scheme_end + 3..].find('/') {
            Some(authority_end) => &path[scheme_end + 3 + authority_end..],
            None => "/",
        },
        None => path,
    }
}

/// The index, in `/endpoints/{EndpointName}/<operation>`, of the segment the two
/// SageMaker Runtime endpoints' modelled URIs differ at.
const RUNTIME_INVOCATION_SEGMENT_INDEX: usize = 2;

/// That segment's value for the ONE operation the SageMaker Runtime HTTP2 endpoint binds.
///
/// Source: `smithy_query(query="digest")` on both models — HTTP2 binds only
/// `POST /endpoints/{EndpointName}/invocations-bidirectional-stream`, while SageMaker
/// Runtime binds `…/invocations`, `…/async-invocations` and
/// `…/invocations-response-stream` under the same prefix.
const RUNTIME_BIDIRECTIONAL_STREAM_SEGMENT: &str = "invocations-bidirectional-stream";

/// Whether `path` addresses the bidirectional-stream invocation — the one operation that
/// separates the two emulated SageMaker Runtime endpoints.
///
/// Those two share their SigV4 signing name, their `runtime.sagemaker` endpointPrefix AND
/// their first URI path segment (`endpoints`), so [`claims_request`] alone leaves both
/// claiming each other's traffic. The HTTP2 endpoint claims a path this answers `true`
/// for; its sibling claims the complement. Both sides read this ONE predicate rather
/// than a constant of their own, so the two claims cannot drift into zero or two
/// claimants for the space they share.
pub fn addresses_runtime_bidirectional_stream(path: &str) -> bool {
    nth_path_segment(path, RUNTIME_INVOCATION_SEGMENT_INDEX)
        == Some(RUNTIME_BIDIRECTIONAL_STREAM_SEGMENT)
}

/// The `<Service>` of an `rpcv2Cbor` URI `/service/<Service>/operation/<Operation>`,
/// or `None` for any other path.
fn rpcv2_cbor_service_named(path: &str) -> Option<&str> {
    let rest = path.trim_start_matches('/').strip_prefix("service/")?;
    let (service, operation) = rest.split_once("/operation/")?;
    (!service.is_empty() && !service.contains('/') && !operation.is_empty()).then_some(service)
}

/// The local name of a Smithy shape id: `com.amazonaws.marketplacemetering#AWSMPMeteringService`
/// -> `AWSMPMeteringService`. An id with no namespace is already a local name.
pub fn shape_local_name(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// Strip a trailing `_<digits>` API-version suffix, e.g. `DynamoDB_20120810` ->
/// `DynamoDB`, which is what `emulator_server::service_from_target` does to the target
/// it reads. Leaves any other name intact (`AWSMPCommerceService_v20200301` keeps its
/// suffix: `v20200301` is not digits).
fn strip_version_suffix(name: &str) -> &str {
    match name.rfind('_') {
        Some(index) => {
            let suffix = &name[index + 1..];
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                &name[..index]
            } else {
                name
            }
        }
        None => name,
    }
}

#[cfg(test)]
mod tests {
    //! The claim rule over the shapes a shared signing name produces: two REST peers
    //! with disjoint path spaces, RPC peers that all post to one path and are
    //! separated only by the service they name, and two REST peers that share a path
    //! space and are separated by a deeper segment.

    use super::*;

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// Which SigV4 scheme a request presents its credential under decides where the key
    /// id is read from, and nothing else: the `Authorization` header when it carries one,
    /// the presigned `X-Amz-Credential` scope otherwise, and the framework's own sentinel
    /// when the request presents neither.
    ///
    /// The last row also PINS that sentinel against the framework
    /// ([`UNSIGNED_ACCESS_KEY_ID`] restates a value `extract_akid` does not export), so a
    /// change to it fails here instead of silently turning an unsigned request into a
    /// caller named `default`.
    #[test]
    fn a_credential_is_read_from_whichever_sigv4_scheme_presented_it() {
        let presigned = |query: &str| {
            http::Request::builder()
                .method("GET")
                .uri(format!("https://sts.amazonaws.com/?{query}"))
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0
        };
        let unsigned = presigned("Action=GetCallerIdentity");
        assert_eq!(
            ::emulator_server::extract_akid(&unsigned.headers),
            UNSIGNED_ACCESS_KEY_ID,
            "the framework's sentinel for a request carrying no credential has changed"
        );

        // (case, the request, the key id it presents)
        let rows: Vec<(&str, http::request::Parts, &str)> = vec![
            (
                "a header-signed request",
                parts("sts", "/", None),
                "AKIDEXAMPLE",
            ),
            (
                "a presigned request whose scope separators are encoded",
                presigned(
                    "Action=GetCallerIdentity&X-Amz-Credential=\
                     ASIAPRESIGNEDKEY0001%2F20260101%2Fus-east-1%2Fsts%2Faws4_request",
                ),
                "ASIAPRESIGNEDKEY0001",
            ),
            (
                "a presigned request whose scope separators are literal",
                presigned(
                    "Action=GetCallerIdentity&X-Amz-Credential=\
                     ASIAPRESIGNEDKEY0002/20260101/us-east-1/sts/aws4_request",
                ),
                "ASIAPRESIGNEDKEY0002",
            ),
            (
                "a request carrying neither",
                unsigned,
                UNSIGNED_ACCESS_KEY_ID,
            ),
        ];

        for (case, request, expected) in rows {
            let actual = request_access_key_id(&request);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the key id {expected:?}, got {actual:?}"
            );
        }
    }

    fn parts(signing_name: &str, uri: &str, target: Option<&str>) -> http::request::Parts {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", authorization(signing_name));
        if let Some(target) = target {
            builder = builder.header("x-amz-target", target);
        }
        builder.body(()).expect("a well-formed test request").into_parts().0
    }

    /// Two REST services sharing `aws-marketplace`: each one's own path space reaches
    /// it and the other's does not, and a request signed for another service reaches
    /// neither.
    #[test]
    fn rest_peers_claim_only_their_own_path_space() {
        const CATALOG: &[&str] = &["CancelChangeSet", "ListAssessments"];
        const REPORTING: &[&str] = &["getBuyerDashboard"];

        // (case, signing name, uri, claimed by catalog, claimed by reporting)
        let rows: [(&str, &str, &str, bool, bool); 5] = [
            ("catalog path", "aws-marketplace", "/CancelChangeSet", true, false),
            ("catalog list path", "aws-marketplace", "/ListAssessments", true, false),
            ("reporting path", "aws-marketplace", "/getBuyerDashboard", false, true),
            ("neither path", "aws-marketplace", "/Nope", false, false),
            ("another service", "s3", "/CancelChangeSet", false, false),
        ];

        for (case, signing_name, uri, by_catalog, by_reporting) in rows {
            let parts = parts(signing_name, uri, None);
            let catalog = claims_request(&parts, "aws-marketplace", CATALOG, None);
            let reporting = claims_request(&parts, "aws-marketplace", REPORTING, None);
            assert_eq!(
                (catalog, reporting),
                (by_catalog, by_reporting),
                "case {case:?}: expected (catalog, reporting) == ({by_catalog}, {by_reporting}), \
                 got ({catalog}, {reporting})"
            );
        }
    }

    /// RPC peers sharing one signing name all post to `/` (or `/service/...`), so only
    /// the service the request NAMES decides.
    #[test]
    fn rpc_peers_claim_only_the_requests_that_name_them() {
        const METERING: &str = "com.amazonaws.marketplacemetering#AWSMPMeteringService";
        const ENTITLEMENT: &str =
            "com.amazonaws.marketplaceentitlementservice#AWSMPEntitlementService";
        // What `emulator-core-build-support` publishes for them: metering (awsJson1_1,
        // no http bindings) gets an EMPTY list, entitlement (awsJson1_1 + rpcv2Cbor)
        // the two protocol sentinels.
        const METERING_SEGMENTS: &[&str] = &[];
        const ENTITLEMENT_SEGMENTS: &[&str] = &["/", "service"];

        // (case, uri, x-amz-target, claimed by metering, claimed by entitlement)
        let rows: [(&str, &str, Option<&str>, bool, bool); 8] = [
            ("metering target", "/", Some("AWSMPMeteringService.MeterUsage"), true, false),
            (
                "entitlement target",
                "/",
                Some("AWSMPEntitlementService.GetEntitlements"),
                false,
                true,
            ),
            (
                "entitlement cbor uri",
                "/service/AWSMPEntitlementService/operation/GetEntitlements",
                None,
                false,
                true,
            ),
            (
                "a third service's cbor uri",
                "/service/AWSMPCommerceService_v20200301/operation/DescribeAgreement",
                None,
                false,
                false,
            ),
            ("a REST peer's path", "/CancelChangeSet", None, false, false),
            (
                // A request that names one service while addressing another's REST
                // path contradicts itself: it reaches nobody, never two claimants.
                "a target on a REST peer's path",
                "/CancelChangeSet",
                Some("AWSMPMeteringService.MeterUsage"),
                false,
                false,
            ),
            (
                "a cbor uri naming it, with its own operation",
                "/service/AWSMPMeteringService/operation/MeterUsage",
                None,
                true,
                false,
            ),
            ("root post naming nobody", "/", None, false, false),
        ];

        for (case, uri, target, by_metering, by_entitlement) in rows {
            let parts = parts("aws-marketplace", uri, target);
            let metering =
                claims_request(&parts, "aws-marketplace", METERING_SEGMENTS, Some(METERING));
            let entitlement = claims_request(
                &parts,
                "aws-marketplace",
                ENTITLEMENT_SEGMENTS,
                Some(ENTITLEMENT),
            );
            assert_eq!(
                (metering, entitlement),
                (by_metering, by_entitlement),
                "case {case:?}: expected (metering, entitlement) == ({by_metering}, \
                 {by_entitlement}), got ({metering}, {entitlement})"
            );
        }
    }

    /// An RPC model that ALSO binds operations to URIs keeps both claims.
    #[test]
    fn a_mixed_model_claims_its_literal_paths_and_its_rpc_form() {
        const SHAPE: &str = "com.amazonaws.glue#AWSGlue";
        const SEGMENTS: &[&str] = &["/", "dev-endpoint"];

        // (case, uri, target, claimed)
        let rows: [(&str, &str, Option<&str>, bool); 5] = [
            ("its own rpc form", "/", Some("AWSGlue.GetTables"), true),
            ("its own bound path", "/dev-endpoint/x", None, true),
            ("another path", "/CancelChangeSet", None, false),
            ("another service's rpc form", "/", Some("AWSMPMeteringService.MeterUsage"), false),
            (
                // Its own path, but naming another service out of band: the request
                // contradicts itself, so it reaches nobody rather than two claimants.
                "its own bound path, naming another service",
                "/dev-endpoint/x",
                Some("AWSMPMeteringService.MeterUsage"),
                false,
            ),
        ];

        for (case, uri, target, expected) in rows {
            let parts = parts("glue", uri, target);
            let claimed = claims_request(&parts, "glue", SEGMENTS, Some(SHAPE));
            assert_eq!(
                claimed, expected,
                "case {case:?}: expected claimed == {expected}, got {claimed}"
            );
        }
    }

    /// A target prefix carrying DOTS of its own names the same service as the bare shape
    /// name: the service is the last segment of the prefix, not the first.
    ///
    /// Read at the first dot — the framework's own `service_from_target` — every row
    /// below names the namespace label `com`, which is no service, so a service whose
    /// SDK sends a fully qualified prefix claimed none of its own traffic.
    #[test]
    fn an_rpc_service_claims_a_dotted_target_prefix() {
        const CODECONNECTIONS: &str = "com.amazonaws.codeconnections#CodeConnections_20231201";
        const CLOUDTRAIL: &str = "com.amazonaws.cloudtrail#CloudTrail_20131101";

        // (case, declared shape id, target, expected)
        let rows: [(&str, &str, &str, bool); 10] = [
            (
                "the absolute shape id an SDK sends as the prefix",
                CODECONNECTIONS,
                "com.amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink",
                true,
            ),
            (
                "the fully qualified Coral name, with a version label in the namespace",
                CLOUDTRAIL,
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                true,
            ),
            (
                "the bare shape name, which the first-dot reader already routed",
                CODECONNECTIONS,
                "CodeConnections_20231201.DeleteRepositoryLink",
                true,
            ),
            (
                "one namespace label before the shape name",
                CODECONNECTIONS,
                "codeconnections.CodeConnections_20231201.GetHost",
                true,
            ),
            (
                "the version suffix dropped by the client",
                CODECONNECTIONS,
                "com.amazonaws.codeconnections.CodeConnections.GetHost",
                true,
            ),
            (
                "another service's dotted prefix",
                CODECONNECTIONS,
                "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.CreateTrail",
                false,
            ),
            (
                "a dotted prefix whose last segment merely ends in the shape name",
                CODECONNECTIONS,
                "com.amazonaws.codeconnections.NotCodeConnections_20231201.GetHost",
                false,
            ),
            (
                // A header that is present but names nothing decides the claim on its
                // own: the request is addressed in the awsJson form, so the `rpcv2Cbor`
                // URI is not consulted for it, and it names no service.
                "an empty target header",
                CODECONNECTIONS,
                "",
                false,
            ),
            (
                "a target header that is only a prefix",
                CODECONNECTIONS,
                "com.amazonaws.codeconnections.",
                true,
            ),
            (
                "a target header that names neither a service nor an operation",
                CODECONNECTIONS,
                "...",
                false,
            ),
        ];

        for (case, shape_id, target, expected) in rows {
            let parts = parts("codeconnections", "/", Some(target));
            let claimed = claims_request(&parts, "codeconnections", &[], Some(shape_id));
            assert_eq!(
                claimed, expected,
                "case {case:?}: expected claimed == {expected} for target {target:?}, got \
                 {claimed}"
            );
        }
    }

    #[test]
    fn a_version_suffixed_target_names_the_same_service() {
        // (case, declared shape id, target, expected)
        let rows: [(&str, &str, &str, bool); 3] = [
            (
                "suffix on both",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "DynamoDB_20120810.GetItem",
                true,
            ),
            (
                "suffix on the model only",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "DynamoDB.GetItem",
                true,
            ),
            (
                "a different service",
                "com.amazonaws.dynamodb#DynamoDB_20120810",
                "AWSKMS.Encrypt",
                false,
            ),
        ];

        for (case, shape_id, target, expected) in rows {
            let parts = parts("dynamodb", "/", Some(target));
            let claimed = claims_request(&parts, "dynamodb", &[], Some(shape_id));
            assert_eq!(
                claimed, expected,
                "case {case:?}: expected claimed == {expected}, got {claimed}"
            );
        }
    }

    /// RULE: where one endpoint and one signing name serve SEVERAL API VERSIONS of a
    /// service, each version claims exactly the targets that name its own service shape —
    /// the version suffix included, because it is the only thing that differs.
    ///
    /// The two shape ids are Kinesis Analytics' (`smithy_query(query="service_info")` on
    /// each model); both models declare `endpointPrefix`/`sigv4Name` `kinesisanalytics`,
    /// so every row routes to that one key and only the target decides.
    #[test]
    fn each_api_version_under_one_signing_name_claims_only_its_own_targets() {
        const V1: &str = "com.amazonaws.kinesisanalytics#KinesisAnalytics_20150814";
        const V2: &str = "com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523";
        const KEY: &str = "kinesisanalytics";

        // (case, target, the version that claims it: Some(shape id), or None for neither)
        let rows: [(&str, &str, Option<&str>); 8] = [
            (
                "v2's own target",
                "KinesisAnalytics_20180523.CreateApplication",
                Some(V2),
            ),
            (
                "v1's own target",
                "KinesisAnalytics_20150814.CreateApplication",
                Some(V1),
            ),
            (
                "an operation only v2 models",
                "KinesisAnalytics_20180523.CreateApplicationSnapshot",
                Some(V2),
            ),
            (
                "an operation only v1 models",
                "KinesisAnalytics_20150814.DescribeApplication",
                Some(V1),
            ),
            (
                "v2's target written out as a Coral name",
                "com.amazonaws.kinesisanalyticsv2.KinesisAnalytics_20180523.StartApplication",
                Some(V2),
            ),
            (
                "v1's target written out as a Coral name",
                "com.amazonaws.kinesisanalytics.KinesisAnalytics_20150814.StartApplication",
                Some(V1),
            ),
            // No version named at all: the target names neither service shape.
            ("the version suffix dropped", "KinesisAnalytics.CreateApplication", None),
            ("another service entirely", "AWSKMS.Encrypt", None),
        ];

        for (case, target, expected) in rows {
            let parts = parts(KEY, "/", Some(target));
            let v1 = claims_versioned_rpc_request(&parts, KEY, &[], V1);
            let v2 = claims_versioned_rpc_request(&parts, KEY, &[], V2);
            let claimants: Vec<&str> = [(V1, v1), (V2, v2)]
                .into_iter()
                .filter_map(|(shape, claimed)| claimed.then_some(shape))
                .collect();
            let expected_claimants: Vec<&str> = expected.into_iter().collect();
            assert_eq!(
                claimants, expected_claimants,
                "case {case:?}: expected target {target:?} to be claimed by \
                 {expected_claimants:?}, got {claimants:?}"
            );
        }
    }

    /// The path of a whole request URI, in each form the seams that read one see: an
    /// absolute URI as an SDK sends it, with and without a query string or a fragment, and
    /// a bare path as a `Parts` already carries.
    #[test]
    fn uri_path_reads_the_path_of_a_whole_uri() {
        // (case, uri, expected path)
        let rows: [(&str, &str, &str); 9] = [
            ("an absolute URI", "https://rum.us-east-1.amazonaws.com/tags/arn", "/tags/arn"),
            (
                "an absolute URI with a query string",
                "https://rum.us-east-1.amazonaws.com/tags/arn?tagKeys=Env",
                "/tags/arn",
            ),
            (
                "an absolute URI with a fragment",
                "https://host/appmonitor/name#frag",
                "/appmonitor/name",
            ),
            ("an authority and nothing else", "https://host", "/"),
            ("an authority and a bare slash", "https://host/", "/"),
            ("a bare path", "/tags/arn", "/tags/arn"),
            ("a bare path with a query string", "/tags/arn?tagKeys=Env", "/tags/arn"),
            ("the root path", "/", "/"),
            ("an empty URI", "", ""),
        ];

        for (case, uri, expected) in rows {
            let actual = uri_path(uri);
            assert_eq!(
                actual, expected,
                "case {case:?}: the path of {uri:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A path label is decoded with PATH semantics: escapes resolved, `+` kept as the
    /// literal character it is on a path, unlike the query decoder the helper is built on.
    #[test]
    fn a_path_label_keeps_its_plus_and_resolves_its_escapes() {
        // (case, label, expected value)
        let rows: [(&str, &str, &str); 8] = [
            ("nothing to decode", "provider-abc1234567", "provider-abc1234567"),
            ("a literal plus is a literal on a path", "c++", "c++"),
            ("an escaped plus decodes to a plus", "a%2Bb", "a+b"),
            ("a literal and an escaped plus together", "a+b%2Bc", "a+b+c"),
            ("an escaped slash", "service-role%2Fname", "service-role/name"),
            ("an escaped colon", "arn%3Aaws", "arn:aws"),
            ("an escaped percent", "100%25", "100%"),
            ("the empty label", "", ""),
        ];

        for (case, label, expected) in rows {
            let actual = percent_decode_path_label(label);
            assert_eq!(
                actual, expected,
                "case {case:?}: decoding {label:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The segment index is the position in the path as written, whatever the leading or
    /// trailing slashes — the reader two peers sharing a path space are separated by.
    #[test]
    fn nth_path_segment_reads_the_position_in_the_path() {
        // (case, path, index, expected)
        let rows: [(&str, &str, usize, Option<&str>); 9] = [
            ("first of three", "/endpoints/ep-1/invocations", 0, Some("endpoints")),
            ("second of three", "/endpoints/ep-1/invocations", 1, Some("ep-1")),
            ("third of three", "/endpoints/ep-1/invocations", 2, Some("invocations")),
            (
                "third of three, the sibling's operation",
                "/endpoints/ep-1/invocations-bidirectional-stream",
                2,
                Some("invocations-bidirectional-stream"),
            ),
            ("past the end", "/endpoints/ep-1", 2, None),
            ("no leading slash", "endpoints/ep-1/invocations", 2, Some("invocations")),
            ("trailing slash leaves an empty segment", "/endpoints/ep-1/", 2, Some("")),
            ("the root path", "/", 0, Some("")),
            ("the empty path", "", 0, Some("")),
        ];

        for (case, path, index, expected) in rows {
            let actual = nth_path_segment(path, index);
            assert_eq!(
                actual, expected,
                "case {case:?}: segment {index} of {path:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The path a service routes on is the URI's path alone: the query string and the
    /// authority of an absolute-form URI are not part of it.
    #[test]
    fn request_path_reads_the_path_out_of_either_uri_form() {
        // (case, uri, expected)
        let rows: [(&str, &str, &str); 8] = [
            ("origin form", "/model/m-1/invoke", "/model/m-1/invoke"),
            (
                "origin form with a query string",
                "/async-invoke?maxResults=10",
                "/async-invoke",
            ),
            (
                "absolute form",
                "https://bedrock-runtime.us-east-1.amazonaws.com/model/m-1/converse",
                "/model/m-1/converse",
            ),
            (
                "absolute form with a query string",
                "https://host.example/async-invoke?nextToken=abc",
                "/async-invoke",
            ),
            ("absolute form with no path", "https://host.example", "/"),
            ("absolute form, root path", "https://host.example/", "/"),
            ("the root path", "/", "/"),
            ("the empty uri", "", ""),
        ];

        for (case, uri, expected) in rows {
            let actual = request_path(uri);
            assert_eq!(
                actual, expected,
                "case {case:?}: {uri:?} expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A modelled `@http` URI pattern is matched segment for segment, with each kind of
    /// placeholder taking what the model says it takes — and a path that differs in any
    /// segment, or in how many there are, is another route.
    #[test]
    fn an_http_uri_pattern_matches_exactly_the_paths_its_labels_admit() {
        // (case, pattern, path, expected). The patterns are real ones: kafkaconnect's
        // `/v1/connectors/{connectorArn}` and `/v1/tags/{resourceArn}`, a nested
        // collection, a bare collection, and a greedy label as S3 binds one
        // (`/{Bucket}/{Key+}`).
        let rows: [(&str, &str, &str, bool); 16] = [
            (
                "a label filled with one segment",
                "/v1/connectors/{connectorArn}",
                "/v1/connectors/arn%3Aaws%3Akafkaconnect%3A%3A%3Aconnector%2Fc%2F1",
                true,
            ),
            (
                "a trailing slash on the request path",
                "/v1/connectors/{connectorArn}",
                "/v1/connectors/c/",
                true,
            ),
            (
                "the label's segment missing",
                "/v1/connectors/{connectorArn}",
                "/v1/connectors",
                false,
            ),
            (
                "an empty label segment",
                "/v1/connectors/{connectorArn}",
                "/v1/connectors//",
                false,
            ),
            (
                "a segment past the label",
                "/v1/connectors/{connectorArn}",
                "/v1/connectors/c/operations",
                false,
            ),
            (
                "another literal in the same position",
                "/v1/connectors/{connectorArn}",
                "/v1/custom-plugins/c",
                false,
            ),
            (
                "another version prefix",
                "/v1/connectors/{connectorArn}",
                "/v2/connectors/c",
                false,
            ),
            (
                "the tag route's label",
                "/v1/tags/{resourceArn}",
                "/v1/tags/arn",
                true,
            ),
            (
                "a literal after a label",
                "/v1/connectors/{connectorArn}/operations",
                "/v1/connectors/c/operations",
                true,
            ),
            (
                "that literal spelled otherwise",
                "/v1/connectors/{connectorArn}/operations",
                "/v1/connectors/c/restart",
                false,
            ),
            (
                "a collection with no label",
                "/v1/connectors",
                "/v1/connectors",
                true,
            ),
            (
                "a query literal in the pattern, which selects and does not match",
                "/v1/connectors?list-type=2",
                "/v1/connectors",
                true,
            ),
            (
                "a greedy label taking one segment",
                "/{Bucket}/{Key+}",
                "/bucket/key",
                true,
            ),
            (
                "a greedy label taking several",
                "/{Bucket}/{Key+}",
                "/bucket/a/b/c",
                true,
            ),
            ("a greedy label taking none", "/{Bucket}/{Key+}", "/bucket", false),
            ("the root path against the root pattern", "/", "/", true),
        ];

        for (case, pattern, path, expected) in rows {
            let actual = path_matches_http_uri(pattern, path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {pattern:?} against {path:?} expected {expected}, got {actual}"
            );
        }
    }

    /// The one predicate both SageMaker Runtime endpoints read: true for exactly the
    /// invocation the HTTP2 one binds, false for every other path in the space they
    /// share, so the two complementary claims cannot leave a path with zero or two
    /// claimants.
    #[test]
    fn the_bidirectional_stream_invocation_is_told_from_every_other_invocation() {
        // (case, path, expected) — the four invocations the two models bind
        // (`smithy_query(query="digest")` on each), plus paths around them.
        let rows: [(&str, &str, bool); 8] = [
            (
                "the HTTP2 endpoint's own invocation",
                "/endpoints/ep-1/invocations-bidirectional-stream",
                true,
            ),
            ("the sibling's invocations", "/endpoints/ep-1/invocations", false),
            (
                "the sibling's async invocations",
                "/endpoints/ep-1/async-invocations",
                false,
            ),
            (
                "the sibling's response stream",
                "/endpoints/ep-1/invocations-response-stream",
                false,
            ),
            (
                "an invocation neither model binds",
                "/endpoints/ep-1/no-such-invocation",
                false,
            ),
            ("no invocation segment at all", "/endpoints/ep-1", false),
            (
                "the segment at the wrong depth",
                "/endpoints/invocations-bidirectional-stream",
                false,
            ),
            (
                "an endpoint name that looks like the segment",
                "/endpoints/invocations-bidirectional-stream/invocations",
                false,
            ),
        ];

        for (case, path, expected) in rows {
            let actual = addresses_runtime_bidirectional_stream(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: {path:?} expected {expected}, got {actual}"
            );
        }
    }

    /// The route half of [`claims_modeled_route`] is [`addresses_modeled_route`], which
    /// reads a service's OWN generated protocol and routing table; it is exercised
    /// end-to-end against the real models in
    /// `core/router/tests/all/bedrock_agentcore_signing_name_routing.rs` and the owning
    /// crates' own suites, where those tables exist. What is unit-testable here is the
    /// KEY half, below.
    #[allow(dead_code)]
    fn route_half_is_covered_by_the_router_suites() {}

    /// An endpoint answers on every key that names it: its signing name, and the
    /// `endpointPrefix` an unsigned, host-addressed request resolves to.
    #[test]
    fn an_endpoint_answers_on_its_signing_name_and_on_its_endpoint_prefix() {
        /// The keys ONE endpoint answers on: the signing name it shares with its peer,
        /// and its own (longer) endpoint prefix.
        const KEYS: &[&str] = &["shared", "shared-control"];

        // (case, authorization scope, host, expected)
        let rows: [(&str, Option<&str>, Option<&str>, bool); 7] = [
            ("the shared signing name, signed", Some("shared"), None, true),
            (
                "its own endpoint prefix, signed for it",
                Some("shared-control"),
                None,
                true,
            ),
            (
                "its own endpoint prefix, host-addressed and unsigned",
                None,
                Some("shared-control.us-east-1.amazonaws.com"),
                true,
            ),
            (
                "the shared signing name as a host, unsigned",
                None,
                Some("shared.us-east-1.amazonaws.com"),
                true,
            ),
            (
                "another service's host, unsigned",
                None,
                Some("lambda.us-east-1.amazonaws.com"),
                false,
            ),
            (
                // The credential scope outranks the host, so a request signed for
                // another service does not reach this endpoint however it is addressed.
                "another service's scope on this endpoint's host",
                Some("lambda"),
                Some("shared-control.us-east-1.amazonaws.com"),
                false,
            ),
            ("no routing signal at all", None, None, false),
        ];

        for (case, scope, host, expected) in rows {
            let mut builder = http::Request::builder().method("POST").uri("/widgets/");
            if let Some(scope) = scope {
                builder = builder.header("authorization", authorization(scope));
            }
            if let Some(host) = host {
                builder = builder.header("host", host);
            }
            let parts = builder
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0;
            let actual = routes_to_any(&parts, KEYS);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected routes_to_any == {expected}, got {actual}"
            );
        }
    }

    /// The two properties a derived, wire-visible id needs: the same parts mix
    /// to the same 16 hexadecimal digits every time, and parts that differ —
    /// including parts that only differ in where one ends — mix to different
    /// ids.
    #[test]
    fn a_stable_id_is_reproducible_and_separates_its_parts() {
        // (case, parts)
        let rows: [(&str, &[&str]); 6] = [
            ("nothing", &[]),
            ("one empty part", &[""]),
            ("one part", &["inferred"]),
            ("two parts", &["inferred", "1-58406520-a006649127e371903a2de979"]),
            (
                "three parts",
                &[
                    "inferred",
                    "1-58406520-a006649127e371903a2de979",
                    "0000000000000001",
                ],
            ),
            // The same characters, split differently: a mixing function that
            // concatenated its parts would answer the same id for this row as
            // for the one above.
            (
                "the same characters split differently",
                &[
                    "inferred",
                    "1-58406520-a006649127e371903a2de9790000000000000001",
                ],
            ),
        ];

        let mut seen: Vec<(&str, String)> = Vec::new();
        for (case, parts) in rows {
            let id = stable_hex_id(parts);
            assert_eq!(
                id.len(),
                16,
                "case {case:?}: an id is 16 hexadecimal digits, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case:?}: an id is hexadecimal, got {id:?}"
            );
            assert_eq!(
                stable_hex_id(parts),
                id,
                "case {case:?}: the same parts must mix to the same id"
            );
            if let Some((other, _)) = seen.iter().find(|(_, other)| *other == id) {
                panic!("case {case:?}: mixes to the same id as case {other:?}: {id:?}");
            }
            seen.push((case, id));
        }
    }

    /// Whatever 16 bytes a service hands [`uuid_from_random_bytes`], the text it gets
    /// back is a version-4 UUID in the canonical hyphenated form: five lowercase-hex
    /// groups of 8-4-4-4-12, the version nibble and the variant bits set, and the
    /// remaining 122 bits exactly the ones that were handed in.
    ///
    /// The rows differ along the bytes, which is what the rendering generalises over:
    /// all zero, all ones, the bytes whose version and variant nibbles are already
    /// right, bytes whose nibbles are wrong in every position, and an ascending run.
    #[test]
    fn sixteen_bytes_render_as_a_canonical_version_4_uuid() {
        // (case, bytes)
        let rows: [(&str, [u8; 16]); 5] = [
            ("all zero", [0x00; 16]),
            ("all ones", [0xff; 16]),
            (
                "version and variant nibbles already set",
                [
                    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0x4c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67,
                    0x89, 0xab, 0xcd,
                ],
            ),
            (
                "version and variant nibbles set wrong",
                [
                    0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0xff, 0x33, 0xff, 0x55, 0x66, 0x77, 0x88,
                    0x99, 0xaa, 0xbb,
                ],
            ),
            (
                "an ascending run",
                [
                    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
                    0x0d, 0x0e, 0x0f,
                ],
            ),
        ];

        for (case, bytes) in rows {
            let rendered = uuid_from_random_bytes(bytes);

            let groups: Vec<&str> = rendered.split('-').collect();
            assert_eq!(
                groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12],
                "case {case:?}: a UUID is five groups of 8-4-4-4-12, got {rendered:?}"
            );
            assert!(
                groups.iter().all(|group| group
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())),
                "case {case:?}: a UUID is lowercase hexadecimal, got {rendered:?}"
            );

            let parsed = uuid::Uuid::parse_str(&rendered)
                .unwrap_or_else(|error| panic!("case {case:?}: {rendered:?} is no UUID: {error}"));
            assert_eq!(
                parsed.get_version_num(),
                4,
                "case {case:?}: {rendered:?} must carry the version-4 nibble"
            );
            assert_eq!(
                parsed.get_variant(),
                uuid::Variant::RFC4122,
                "case {case:?}: {rendered:?} must carry the variant bits, got {:?}",
                parsed.get_variant()
            );

            // Only the version and variant bits are the layout's; every other bit is
            // the one the caller supplied.
            let mut expected = bytes;
            expected[6] = (expected[6] & 0x0f) | 0x40;
            expected[8] = (expected[8] & 0x3f) | 0x80;
            assert_eq!(
                parsed.as_bytes(),
                &expected,
                "case {case:?}: the rendered UUID must carry the supplied bytes, got {rendered:?}"
            );
        }
    }
}
