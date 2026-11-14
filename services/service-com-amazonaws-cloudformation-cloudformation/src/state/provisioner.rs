// SPDX-License-Identifier: CC0-1.0

//! One entry per resource type this emulator really creates: which service owns the type,
//! and the create, update and delete calls that make its resources exist, change and stop
//! existing.
//!
//! This is the half of provisioning that is PER RESOURCE TYPE. The half that is not —
//! reconciling what a deployment declares against what the stack already has, deleting
//! what it replaced, and the in-process transport every one of these calls rides on —
//! is [`super::provisioning`], which owns the algorithm and reaches this module only
//! through [`provisioner_for`]. Adding a type is therefore a change to THIS file alone:
//! one arm in [`provisioner_for`] and the create/delete pair it names.
//!
//! A type with no entry here is left declared-only, which
//! [`super::provisioning`](crate::state::provisioning) documents: nothing is created in
//! any service, and the resource still carries the physical id and attributes
//! CloudFormation's documented naming rules give it.
//!
//! A deployment that re-declares a resource with a DIFFERENT configuration is applied
//! through the owning service's own update API ([`Provisioner::update`]), which is per
//! type for the same reason the create is: `AWS::SQS::Queue`'s configuration is its
//! attributes and its tags, so the change is a `SetQueueAttributes` plus a
//! `TagQueue`/`UntagQueue` ([`update_queue`]). A type with no arm there has nothing asked
//! of its service and the deployment reports the change as not applied, exactly as a type
//! with no create arm is left declared-only — never a stack that completed while the
//! caller's change went nowhere.
//!
//! How a create call is BUILT depends on what the type's properties are:
//!
//! * `AWS::KinesisFirehose::DeliveryStream`'s properties ARE `CreateDeliveryStream`'s own
//!   members, so the evaluated property document is the request, passed through verbatim
//!   with the one member CloudFormation decides (the name) set on the way out. That create
//!   is asynchronous and leaves the stream in a status in which it takes no data, so the
//!   resource's creation is held open until Amazon Data Firehose reports the stream `ACTIVE`
//!   ([`delivery_stream_readiness`]);
//! * `AWS::Glue::Job`'s are `CreateJob`'s own members the same way, with its tags the map
//!   that call takes; the `JobUpdate` that call's sibling `UpdateJob` "completely
//!   overwrites" a job definition with is that same document less the job's name, so one
//!   declaration serves this type's create and its in-place update alike;
//! * `AWS::CloudWatch::MetricStream`'s are `PutMetricStream`'s own members the same way,
//!   and that one call is documented "Creates or updates a metric stream" — so it is also
//!   this type's UPDATE, the one arm here that can apply a changed declaration to a
//!   resource the stack already has;
//! * `AWS::CloudWatch::Dashboard`'s three properties are `PutDashboard`'s own members,
//!   with the dashboard's name the one CloudFormation decides, its `DashboardBody` taken as
//!   the JSON TEXT the request carries, and its `Tags` the declaration's own tag list beside
//!   the ones CloudFormation propagates. That one call is documented "Creates a dashboard if
//!   it does not already exist, or updates an existing dashboard" — so it is this type's
//!   update as well as its create. Its `Tags` member applies on the CREATE alone, that
//!   member being documented as ignored when the call updates a dashboard that exists, so a
//!   later deployment's tag change reaches CloudWatch through that service's own
//!   `TagResource`/`UntagResource` ([`Provisioner::retag`]);
//! * `AWS::CodePipeline::Pipeline`'s are the same document with one transformation: they
//!   are `PipelineDeclaration`'s members spelled with an upper-case initial, so
//!   [`camel_cased`] lower-cases each key's first letter and the document is the
//!   declaration;
//! * `AWS::ECR::PublicRepository`'s are NOT (`RepositoryName` and
//!   `RepositoryCatalogData.RepositoryDescription` against the API's `repositoryName` and
//!   `catalogData.description`), so its arm maps the members AWS's resource reference
//!   publishes one by one, and sends nothing for a property the template leaves out;
//! * the `AWS::OpenSearchServerless::*` types are the same case (`Name`/`Type`/`Policy`
//!   against `name`/`type`/`policy`), and a collection's identifier is one the OWNING
//!   service mints rather than a name the template asked for, so its arm answers that
//!   identifier and [`super::template::identity_is_service_assigned`] names the type it
//!   belongs to;
//! * `AWS::S3::Bucket`'s and `AWS::IAM::Role`'s reach services whose wire is not JSON at
//!   all, so those arms go through the official `aws-sdk-s3` / `aws-sdk-iam` clients over
//!   the same in-process callback ([`s3_client`], [`iam_client`]) and name the members
//!   they send one by one;
//! * `AWS::ECS::Cluster`'s are the same members under a different CASE — the Amazon ECS
//!   model spells them in lower camel case, and renames one (`ClusterSettings` sets
//!   `settings`) — so its arm names the properties `CreateCluster` takes and re-cases each
//!   value;
//! * `AWS::SQS::Queue`'s properties are the queue's ATTRIBUTES, named identically on both
//!   sides, so its arm passes them through as `CreateQueue`'s `Attributes` map, lifting
//!   out the two that are not attributes (the name CloudFormation decides, and the tag
//!   list the create takes as its own member);
//! * `AWS::KMS::Key`'s properties are `CreateKey`'s own members but for five: the key
//!   policy, which that call takes as the document's TEXT, and the four no member of it
//!   carries at all — the rotation of the key's material and whether the key is enabled,
//!   each applied through the KMS call that owns it, the rotation period that first one
//!   takes, and the waiting period the key's deletion is scheduled with;
//! * `AWS::KMS::Alias`'s two properties ARE `CreateAlias`'s own members, named identically
//!   on both sides: the alias name, which is this resource's physical id and so
//!   CloudFormation's to decide, and the `TargetKeyId` the alias is bound to — which the
//!   declaration states as a reference to the key, so what reaches KMS is the identifier
//!   KMS minted for it;
//! * `AWS::SageMaker::ModelPackageGroup`'s are its create's members EXCEPT ONE — a model
//!   group's resource policy is attached through SageMaker's own
//!   `PutModelPackageGroupPolicy` — so its arm passes the properties through and then makes
//!   that second call, the same pair a client makes;
//! * `AWS::SNS::Topic`'s are the topic's attributes in the same way, with the three that
//!   are not attributes lifted out (the name, the tag list and the data-protection policy,
//!   each a `CreateTopic` member of its own), the inline `Subscription` list made by
//!   `Subscribe` calls on the created topic, and `DeliveryStatusLogging` written as the
//!   per-protocol feedback attributes SNS names. A changed declaration reaches the topic
//!   that exists through the one call that writes an attribute of a live one
//!   ([`update_topic`]), its tags through that service's own tagging calls
//!   ([`retag_topic`]);
//! * `AWS::FIS::ExperimentTemplate`'s are the same members in lower camel case, but its
//!   `Targets` and `Actions` are MAPS keyed by names the template chose (as are a target's
//!   `ResourceTags` and `Parameters` and an action's `Parameters` and `Targets`), so its
//!   arm maps each modelled structure member by member and passes every caller-keyed map
//!   through as written. Its physical id is one the owning service ASSIGNS: FIS mints the
//!   experiment template's id, so that id — not a computed name — is what the stack reports
//!   for the resource;
//! * `AWS::WAFRegional::IPSet`'s two properties reach AWS WAF Classic through TWO calls,
//!   because that is how its API takes them: `CreateIPSet` takes the `Name`, each declared
//!   `IPSetDescriptors` entry is INSERTed by an `UpdateIPSet`, and both carry a change
//!   token of their own. Its identity is its owning service's too ("Ref returns the IPSet
//!   ID"), so its arm answers the `IPSetId` AWS WAF returned as the resource's physical id.
//! * the four `AWS::WAFv2::*` types — a web ACL, a rule group, an IP set and a regex
//!   pattern set — have properties that ARE their create's own members, named identically on
//!   both sides, so the evaluated document is the request with the name CloudFormation
//!   decides set on it. The few members the two sides spell DIFFERENTLY are rewritten both
//!   ways ([`wafv2_spelled`]): a byte-match statement's search value is a BLOB, which the
//!   wire carries base64-encoded while a template declares the bytes, and the ARN a
//!   reference statement names another entity by is `ARN` on the wire and `Arn` in a
//!   template. Each
//!   type's identity is WAF's own — its create assigns the entity `Id` and builds the ARN
//!   that carries the scope, the name and that id — so its arm answers both, and what the
//!   stack publishes for the resource is what `AssociateWebACL` and the `Get…` calls take.
//! * `AWS::WAFv2::WebACLAssociation` is none of those: it is no entity of WAF's own but the
//!   BINDING between a protected resource and one of those web ACLs, so its two properties
//!   are `AssociateWebACL`'s own two members and that one call is its whole deployment
//!   ([`associate_web_acl`]) — with the pair it bound as the resource's physical id, which
//!   is what makes `Ref` name which resource is protected by which web ACL.
//! * `AWS::Grafana::Workspace`'s properties are `CreateWorkspace`'s own members under
//!   DIFFERENT names — the type spells the workspace's own fields without the `workspace`
//!   prefix that API carries (`Name`/`Description`/`RoleArn`/`DataSources` against
//!   `workspaceName`/`workspaceDescription`/`workspaceRoleArn`/`workspaceDataSources`) —
//!   so its arm names the members the resource reference publishes one by one and sends
//!   nothing for a property the template leaves out. Its identity is that service's as
//!   well: Amazon Managed Grafana mints the workspace id its `Ref` returns.
//! * `AWS::EMR::Cluster`'s are mostly `RunJobFlow`'s own members (`ReleaseLabel`,
//!   `Applications`, `BootstrapActions`, `Tags`, the roles and the scalars), so they pass
//!   through verbatim; the two that are shaped differently are mapped — the per-role
//!   capacity properties of `Instances` onto the one `InstanceGroups` or `InstanceFleets`
//!   list the API takes, and a `Configurations` entry's `ConfigurationProperties` onto the
//!   API's `Properties`. Its identity is its owning service's as well ("Ref returns the
//!   cluster ID"), so its arm answers the `j-` job-flow id Amazon EMR minted as the
//!   resource's physical id;
//! * `AWS::GuardDuty::Detector`'s are the `CreateDetector` request's own members spelled
//!   with an initial capital (`Enable` / `enable`), all the way down, so its arm carries the
//!   whole evaluated document through the same re-casing [`lower_camel_members`] the ECS
//!   arm needs and lifts out the tag list, which the create takes as a map. A detector's id
//!   is GuardDuty's OWN to mint — `CreateDetector` takes no identifier and answers with
//!   `detectorId` — so, as for the FIS, AWS WAF Classic and Amazon EMR types above, what
//!   the stack reports for the resource is the id that create answered with.
//! * `AWS::GuardDuty::IPSet`'s are `CreateIPSet`'s own members spelled the same way, with
//!   the detector the list hangs off addressing the call rather than travelling in its
//!   body, and the list's NAME taken from the naming rules so a declaration that names none
//!   still creates a list. An IPSet's id is GuardDuty's OWN to mint — `CreateIPSet` takes no
//!   identifier and answers with `ipSetId` — so what the stack reports for the resource is
//!   the id that create answered with, which is what `GetIPSet` resolves.
//! * `AWS::EC2::VPC`'s and `AWS::EC2::Subnet`'s are `CreateVpc`'s and `CreateSubnet`'s own
//!   members under the same names ([`EC2_VPC_CREATE_MEMBERS`],
//!   [`EC2_SUBNET_CREATE_MEMBERS`]), walked against those catalogues by the same
//!   [`query_members`] the Amazon RDS arms use — every one of them a scalar — so a property
//!   neither catalogue holds is REFUSED the way theirs refuse one. Each
//!   type's identity is Amazon EC2's OWN — neither create takes an identifier, and each
//!   answers the `vpc-…`/`subnet-…` id it assigned, which is what that type's `Ref`
//!   returns — so a stack's outputs carry ids EC2 really holds those resources for. The
//!   properties that are not members of either create are each the call that owns them: a
//!   VPC's two DNS switches and a subnet's launch-time switches are `ModifyVpcAttribute`
//!   and `ModifySubnetAttribute`, one attribute per call ("You can only modify one
//!   attribute at a time"), and the tag set is `CreateTags`, the one EC2 call that admits
//!   the reserved `aws:cloudformation:*` keys from a service provisioning on a caller's
//!   behalf. `CreateVpc` answers a VPC `pending` and `DescribeVpcs` reports it `available`
//!   once that service's own clock makes the move due, which is EC2's transition and not a
//!   wait on the creation: the resource the create answered exists and takes the calls made
//!   on it (a subnet is created in it straight away), exactly as the DB instance an
//!   `AWS::RDS::DBInstance` creates is reported created while Amazon RDS still has it
//!   `creating`.
//! * `AWS::EC2::SecurityGroup`'s are `CreateSecurityGroup`'s own members
//!   ([`EC2_SECURITY_GROUP_CREATE_MEMBERS`]) beside the NAME CloudFormation decides, which
//!   that call requires and the type leaves optional, and the two INLINE RULE lists, which
//!   are not members of the create at all: a group is created with no inbound rule and each
//!   declared rule is one permission of the authorize call for its direction
//!   ([`security_group_rule_parameters`]). Its identity is Amazon EC2's own as well — the
//!   create takes no id and answers the `sg-…` one it assigned, which is what "Ref returns
//!   the ID of the security group" names — and its tag set reaches EC2 through the same
//!   `CreateTags` a VPC's does.
//! * `AWS::RDS::DBInstance`'s and `AWS::RDS::DBSubnetGroup`'s are the members
//!   `CreateDBInstance` / `CreateDBSubnetGroup` take, named identically on both sides
//!   except the two Amazon RDS spells differently (`VPCSecurityGroups` is
//!   `VpcSecurityGroupIds`, `CustomIAMInstanceProfile` is `CustomIamInstanceProfile`), so
//!   each arm walks the declaration against its create's member catalogue
//!   ([`CREATE_DB_INSTANCE_MEMBERS`], [`CREATE_DB_SUBNET_GROUP_MEMBERS`], taken from that
//!   service's own model) and writes each member as the awsQuery form carries it — a list
//!   under the element name its list member's `@xmlName` fixes
//!   (`SubnetIds.SubnetIdentifier.1`). A property the catalogue does not carry is REFUSED,
//!   except the few that are members of a DIFFERENT call and are documented as such
//!   ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]). Both identities are CloudFormation's — "Ref
//!   returns the DBInstanceIdentifier" / "the DBSubnetGroupName" — so each arm sets that
//!   member from the resource's physical id, and the DB instance's published endpoint,
//!   ARN and resource id are replaced by the ones Amazon RDS answered with, since those
//!   are the values a client reaches the database through.
//! * `AWS::RDS::DBParameterGroup`'s and `AWS::RDS::DBClusterParameterGroup`'s are their
//!   creates' own members read the same way ([`ParameterGroupCalls`]), with the one property
//!   Amazon RDS spells differently renamed (`Family` is `DBParameterGroupFamily`) and the
//!   name CloudFormation decides set on the way out — each type's `Ref` returns its group's
//!   name. The two declare the same document under different member and action names, so one
//!   arm serves both. Their `Parameters` MAP is no member of either create: a group's
//!   parameter values are written by the modify a client makes next, so the arm makes that
//!   second call ([`modify_parameters`]) in the batches that request's own bound admits —
//!   which is also this type's UPDATE, the modify being the one call of either group's API
//!   that writes a parameter value ([`update_parameter_group`]).
//! * `AWS::Events::Rule`'s are `PutRule`'s own members, spelled identically, with two
//!   exceptions: `EventPattern` is a template DOCUMENT where the request takes the JSON
//!   TEXT of one, and `Targets` is no `PutRule` member at all — a rule's targets are
//!   installed by the `PutTargets` a client makes next, so the arm makes that second call
//!   ([`install_targets`]), maps each declared target against `Target`'s own members and
//!   fails the resource on a target EventBridge reports as a failed entry. `PutRule` is
//!   documented "Creates or updates the specified rule", so it is also this type's UPDATE,
//!   with the declared target list reconciled against the installed one.
//! * `AWS::SSM::Parameter`'s are `PutParameter`'s own members the same way, with the name
//!   CloudFormation decides set on the way out and the declared tag MAP sent as the
//!   `[{Key, Value}]` list that request takes. That one call is "Create or update a
//!   parameter in Parameter Store", so `Overwrite` makes it this type's update too.
//! * `AWS::Cognito::UserPoolGroup`'s are `CreateGroup`'s own members, spelled identically on
//!   both sides (`UserPoolId`, `GroupName`, `Description`, `Precedence`, `RoleArn`), so the
//!   evaluated properties are the request with the one member CloudFormation decides (the
//!   group's name, which is this resource's physical id) set on the way out. A group's
//!   identity is a PAIR — the pool that holds it and its name — so every call of its arm
//!   carries the declared `UserPoolId` beside that name, and a declaration that moves the
//!   group to another pool is no in-place change: `UpdateGroup` changes only the three
//!   configuration members, so such a deployment reports the change as not applied.
//! * the two `AWS::S3Vectors::*` types' properties are their creates' own members with a
//!   lower-case initial, and each type's IDENTITY is its own ARN rather than a name: `Ref`
//!   returns the vector bucket's `bucket/<name>` ARN and the index's
//!   `bucket/<bucket>/index/<name>` one, so the names each create carries are read back off
//!   that ARN ([`vector_bucket_name`], [`vector_index_identity`]) and the delete and the
//!   drift read address the resource by the ARN itself.
//! * the two `AWS::Scheduler::*` types' properties ARE their creates' own members, spelled
//!   identically all the way down, with the name CloudFormation decides carried in the URI
//!   those `restJson1` operations bind it to — so each arm passes the evaluated property
//!   document through, a group's declared tags beside the ones CloudFormation gives it. A
//!   schedule is keyed by its name AND the group holding it
//!   ([`scheduled_group_of`]), so its delete and its drift read carry both. The same
//!   document is what a CHANGED declaration reaches that service as: `UpdateSchedule`
//!   overrides a schedule with every value the request states, and a group's tags — its one
//!   property no call changes on a group that exists — reach it through its own tagging
//!   calls ([`retag_schedule_group`]).
//! * the two `AWS::MSK::*` cluster types' properties are their creates' own members in
//!   lower camel case, so each arm walks the evaluated document ([`msk_members`]) with the
//!   one member whose API spelling is not a case change renamed
//!   ([`MSK_RENAMED_MEMBERS`]); a serverless cluster's two configuration properties go
//!   under the `serverless` body `CreateClusterV2` takes, whose `VpcConfigs` entries are
//!   mapped by name. Each type's identity is the `ClusterArn` Amazon MSK mints ("Ref
//!   returns the ARN of the cluster"), so that ARN — not a computed name — is what the
//!   stack reports for the resource, and the delete and the drift read address the cluster
//!   by it.
//!
//! * `AWS::AmazonMQ::Broker`'s properties ARE `CreateBroker`'s own members with the initial
//!   letter lower-cased, all the way down (a `Users` entry's `Username`/`Password`, a
//!   `Logs`' `General`, a `MaintenanceWindowStartTime`'s `DayOfWeek`), so its arm carries
//!   the evaluated document through with the broker's name — the one value CloudFormation
//!   decides — set on the way out and the declared `[{Key, Value}]` tag list sent as the tag
//!   MAP that create takes. Its identity is Amazon MQ's own ("Ref returns the Amazon MQ
//!   broker ID"): `CreateBroker` takes no identifier and answers the `brokerId` it minted
//!   beside the `brokerArn` it built around that id, so the stack reports that id for the
//!   resource and that ARN as its `Arn`, and the delete and the drift read address the
//!   broker by the id.
//!
//! * the three `AWS::Route53Resolver::*` types' properties ARE their creates' own members,
//!   spelled identically on both sides all the way down, so each arm carries the evaluated
//!   document through ([`resolver_request`]) with the one member CloudFormation decides
//!   added: the `CreatorRequestId` every one of those creates requires and no such type
//!   publishes ([`resolver_creator_request_id`]). Each type's identity is Route 53
//!   Resolver's OWN — `Ref` returns the `rslvr-out-…` endpoint id, the `rslvr-rr-…` rule id
//!   and the `rslvr-rrassoc-…` association id, and none of the creates takes an identifier —
//!   so what the stack reports for the resource is the id that create answered with, and an
//!   association is removed by the rule/VPC PAIR its declaration carries rather than by its
//!   own id.
//!
//! * the three `AWS::Route53Resolver::Firewall*` types are the same case with one difference
//!   each: a DNS Firewall domain list's `Domains` are not members of its create at all —
//!   "creates an EMPTY firewall domain list", which `UpdateFirewallDomains` then populates
//!   ([`create_firewall_domain_list`]) — a rule group's `FirewallRules` are each a
//!   `CreateFirewallRule` against the group the first call made
//!   ([`create_firewall_rule_group`]), and an association's properties ARE
//!   `AssociateFirewallRuleGroup`'s own members ([`associate_firewall_rule_group`]). All
//!   three identities are Route 53 Resolver's own (`rslvr-fdl-…`, `rslvr-frg-…`,
//!   `rslvr-frgassoc-…`), and an association is removed by the id that service minted for
//!   it, which is what `DisassociateFirewallRuleGroup` takes.
//!
//! * the three `AWS::CodeDeploy::*` types' properties are their creates' own members with the
//!   initial letter lower-cased — EXCEPT the ones whose nested structures CodeDeploy ITSELF
//!   spells in upper camel, its `Tag`, `EC2TagFilter` and `TagFilter` shapes each spelling
//!   `Key`, `Value` and `Type` — so every property of those types carries the rule saying
//!   which of the two it is ([`CodeDeployMember`]), and a property no entry names is REFUSED
//!   rather than dropped. A deployment group's `Deployment` property is no member of its
//!   create at all: it is an application revision deployed as soon as the group exists, so it
//!   is a `CreateDeployment` of its own on the group just created — the same second call a
//!   client makes. An application and a deployment configuration are keyed by their names,
//!   which is what each type's `Ref` returns; a group is keyed by the PAIR of the application
//!   and the group name ([`codedeploy_application_of`]), so its delete and its drift read
//!   carry both.
//!
//! * `AWS::EKS::AccessEntry`'s properties are `CreateAccessEntry`'s own members with a
//!   lower-case initial (`KubernetesGroups`, `PrincipalArn`, `Type`, `Username`), with the
//!   cluster carried in the URI that create binds it to and `Tags` sent as that call's tag
//!   MAP. `AccessPolicies` is no member of that create at all: each entry of it is an
//!   `AssociateAccessPolicy` of its own on the entry just created — the same second call a
//!   client makes — so a policy the declaration lists is associated to the entry rather
//!   than dropped. The entry is keyed by the PAIR of the cluster and the principal ARN
//!   ([`access_entry_identity`]), so its delete and its drift read carry both; an
//!   association Amazon EKS refuses leaves no half-configured entry behind, because the
//!   entry this arm just created is removed before the refusal is reported.
//!
//! * the `AWS::ApiGatewayV2::*` types' properties are their creates' own members with the
//!   initial letter lower-cased, all the way down ([`api_gateway_request`],
//!   [`apigatewayv2_child_request`]), with the declared tags sent as those calls' tag MAP
//!   where the type publishes any. The identity of an api, a mapping, an integration, a
//!   route, a deployment and an authorizer is API Gateway v2's OWN — `Ref` returns the id
//!   each create mints, and none of them takes one — so the stack reports what the create
//!   answered, and an api's `ApiEndpoint` attribute is the endpoint that service really
//!   serves it on. A custom domain name is keyed by the name the template states and a
//!   stage by the stage name it states; a mapping is keyed by the PAIR of its domain and the
//!   minted id ([`apigatewayv2_api_mapping_uri`]) and every resource that lives inside an
//!   api by the pair of that api and its own identifier ([`apigatewayv2_child_uri`]), so
//!   their deletes and drift reads carry both. An api declared with an inline OpenAPI
//!   definition is IMPORTED from it ([`import_apigatewayv2_api`]), which is a different call
//!   on a different binding; one declared with a definition held in Amazon S3 is refused
//!   rather than created without it.
//!
//! * `AWS::ApiGateway::DomainName`'s properties are the REST control plane's own
//!   `CreateDomainName` members under the same rule ([`api_gateway_request`]), with its
//!   `Policy` sent as the TEXT of the document a template declares, since that member is "a
//!   stringified JSON policy document". Its identity is the domain name the template states
//!   ("Ref returns the domain name"), so the create is sent that name and the delete and the
//!   drift read address the domain by it; the endpoint names and the ARN the type publishes
//!   are read off the create's answer ([`APIGATEWAY_DOMAIN_NAME_ATTRIBUTES`]), because only
//!   API Gateway assigns them.
//!
//! * `AWS::SES::ConfigurationSet`'s one property that reaches a call of this service is
//!   `TrackingOptions`: a configuration set's own create takes nothing but the name
//!   CloudFormation decides, and the custom open-and-click domain declared beside it is the
//!   `CreateConfigurationSetTrackingOptions` a client makes next on the set just created
//!   ([`create_ses_configuration_set`]). Every other published property of the type reaches
//!   no Amazon SES operation this emulator implements YET, so a declaration carrying one
//!   is REFUSED rather than reported as a set created without it, and a tracking domain
//!   Amazon SES refuses leaves no half-configured set behind. Its identity is
//!   CloudFormation's own — "Ref returns the configuration set name" — so the delete and
//!   the drift read address the set by the resource's physical id.
//!
//! * `AWS::SES::ReceiptRuleSet`'s one property IS the name CloudFormation decides, since
//!   `CreateReceiptRuleSet` takes nothing else ([`create_ses_receipt_rule_set`]), and
//!   `AWS::SES::ReceiptRule`'s three ARE `CreateReceiptRule`'s own members, spelled
//!   identically on both sides (`RuleSetName`, `After`, `Rule`), so the evaluated document
//!   is the request with the one member CloudFormation decides set on the way out: a rule's
//!   `Rule.Name` is REQUIRED by that call, so a declaration that states none has the
//!   resource's own computed name pushed for it ([`create_ses_receipt_rule`]). That name is
//!   what the rule's `Ref` returns, and it is nested inside a property rather than being a
//!   name property of its own, so the arm answers it as the resource's physical id.
//!
//! * `AWS::CodeCommit::Repository`'s properties are `CreateRepository`'s own members with a
//!   lower-case initial (`RepositoryDescription`, `KmsKeyId`), with the repository's name
//!   the one CloudFormation decides and `Tags` sent as that create's tag MAP. `Triggers` is
//!   no member of that create at all: the list it declares IS the repository's trigger
//!   configuration, so it is a `PutRepositoryTriggers` of its own on the repository just
//!   created — the same second call a client makes — and a trigger CodeCommit refuses fails
//!   the resource rather than leaving a repository with no triggers behind. The `Code`
//!   property, which commits a bundle from Amazon S3 into the new repository, is mapped onto
//!   no call here and is REFUSED rather than dropped.
//!
//! * `AWS::Route53::HostedZone`'s properties reach `CreateHostedZone` member by member —
//!   the zone's `Name`, its `HostedZoneConfig`'s comment and the first of the `VPCs` a
//!   private zone answers for, each further one an `AssociateVPCWithHostedZone` of its own —
//!   beside the `CallerReference` every such create requires and the type publishes no
//!   property for ([`route53_caller_reference`]). Its `HostedZoneTags` are no member of
//!   that create at all: a zone is tagged by `ChangeTagsForResource`, in the batches that
//!   call's own bound admits. The zone's identity is Route 53's OWN — "Ref returns the hosted
//!   zone ID", which that create MINTS and no naming rule could compute — so the id it
//!   answered with is what the stack reports for the resource, and that is the value
//!   `GetHostedZone`, `TestDNSAnswer` and a record set's `HostedZoneId` all take.
//!
//! * `AWS::Route53::RecordSet`'s properties ARE the members of the `ResourceRecordSet` a
//!   change batch carries ([`RECORD_SET_MEMBERS`]), spelled identically on both sides, with
//!   one shaped differently — `ResourceRecords` is a list of record VALUES in a template and
//!   a list of `ResourceRecord` structures on the wire — and three that are no members of it:
//!   `HostedZoneId`/`HostedZoneName` say which zone holds the record (the first IS the id,
//!   the second is resolved through `ListHostedZonesByName` as a client resolves it), and
//!   `Comment` is the change batch's own. A record set is created by the `CREATE` action and
//!   removed by the `DELETE` one, which Route 53 matches against the record's own values, so
//!   the delete carries the declaration the create was made from. Its identity is
//!   CloudFormation's ("Ref returns the record name"), so the create answers none of its own.
//!
//! * `AWS::Route53::HealthCheck`'s `HealthCheckConfig` property states the members of the
//!   configuration `CreateHealthCheck` takes ([`HEALTH_CHECK_CONFIG_MEMBERS`]), spelled
//!   identically on both sides, beside the `CallerReference` that create requires and the
//!   type publishes no property for ([`route53_caller_reference`]); its `HealthCheckTags`
//!   reach Route 53 through `ChangeTagsForResource`, as a zone's do. A changed declaration
//!   is applied through `UpdateHealthCheck`, whose members are the editable settings of a
//!   live check, with `ResetElements` for an element the declaration stopped stating. The
//!   check's identity is Route 53's OWN — "Ref returns the health check ID", which that
//!   create MINTS — so the id it answered with is what the stack reports, and that is the
//!   value `GetHealthCheck` and a record set's `HealthCheckId` take.
//!
//! * the three `AWS::Backup::*` types' properties are their creates' own members, with two
//!   differences and no others: a plan's rule list is published as `BackupPlanRule` where
//!   `CreateBackupPlan` takes `Rules`, and a rule's destination vault as
//!   `TargetBackupVault` where it takes `TargetBackupVaultName`
//!   ([`BACKUP_PLAN_MEMBERS`], [`BACKUP_RULE_MEMBERS`]) — so a declared plan body and a
//!   declared selection body reach AWS Backup as the bodies those calls take, and a member
//!   the declaration leaves out is absent from the request, leaving the plan with the
//!   service's own default for it. Each taggable type publishes its tags as a MAP of its
//!   own (`BackupVaultTags`, `BackupPlanTags`), which is the member its create takes them
//!   as ([`backup_declared_tags`]); a selection publishes none. A vault's
//!   `LockConfiguration` is no member of its create at all: it is the
//!   `PutBackupVaultLockConfiguration` a client makes next on the vault just created, and
//!   a lock AWS Backup refuses leaves no unlocked vault behind. A vault is keyed by the
//!   name the template states ("Ref returns the backup vault name"), while a plan's and a
//!   selection's identifiers are AWS Backup's OWN — neither create takes one, and each
//!   answers the id it minted — so what the stack reports for one is that id, which is
//!   what `GetBackupPlan`, `ListBackupSelections` and `GetBackupSelection` resolve; a
//!   selection is addressed by the PAIR of its plan and that id
//!   ([`backup_selection_uri`]), so its delete and its drift read carry both.
//!
//! ## The tags every created resource carries
//!
//! Beside what its declaration asks for, a resource CloudFormation creates carries the tags
//! CloudFormation gives it: the stack-level `Tags` plus `aws:cloudformation:stack-name`,
//! `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id`
//! ([`super::provisioning::StackProvenance`]). That SET is the same for every type, so it is
//! computed once there; APPLYING it is per type, because each owning API takes a tag set its
//! own way — a `Tags` member of the create whose `Tag` shape spells `Key`/`Value` (Amazon
//! SNS, Amazon Data Firehose, Amazon ECR Public, Amazon CloudWatch, Amazon EMR, Amazon
//! SageMaker, IAM), the
//! same list spelled `key`/`value`
//! (Amazon ECS, CodePipeline), a map of key to value (Amazon SQS, AWS FIS), or a tagging
//! call of its own where the create takes no tags at all (Amazon S3's `PutBucketTagging`).
//! [`tags_of`] is the one place the declared and the propagated halves are merged, and a key
//! the declaration itself states keeps the declaration's value. A type that is not taggable
//! — `AWS::WAFRegional::IPSet`, which publishes no `Tags` property — is tagged with nothing,
//! because CloudFormation propagates no tags to one, and a type whose owning API refuses the
//! reserved `aws:` namespace on the path this provisioning call is made over carries the
//! stack's own tags without the three ([`Provisioner::applicable_tags`]).
//!
//! No arm invents a value: a property the template does not declare is absent from the
//! request, and what the owning service answers is what the stack records. A property an
//! arm does not map onto the create's request at all is REFUSED rather than dropped, so a
//! stack never reports a resource configured differently from its declaration — the FIS
//! arm's `ExperimentReportConfiguration` and the ACM certificate arm's
//! `DomainValidationOptions.HostedZoneId` are the two such properties today. Inside a
//! nested structure the member tables are the type's published members, so a member not in
//! one is not a member of the type.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

// The modules and free items of `crate::state` that the per-type arms of this module reach
// by their `super::` path. The arms live in this module's own files
// ([`data_and_storage`] and the ones beside it), so `super::` resolves HERE rather than in
// `crate::state`: re-exporting the names under this module is what keeps one path spelling
// for them across every file of the provisioner, rather than each file importing its own.
// Some are reached only by those files' doc links, which is why the unused-import lint is
// allowed here: dropping them would leave those links pointing at nothing.
#[allow(unused_imports)]
pub(super) use super::{
    drift, dynamic_references, provisioning, scalar_text, service_role, template, DEFAULT_REGION,
};

use super::provisioning::{
    call_json_rpc, call_query, call_rest_json, call_rest_json_named, call_rest_xml,
    percent_encoded, xml_element, xml_elements,
    xml_entry_map, xml_leaf, xml_member_structures, xml_scalar_children, Created, Readiness,
    Refusal, Removal,
    AWS_JSON_1_0, AWS_JSON_1_1,
};
use super::template::{
    ACMPCA_CERTIFICATE, ACMPCA_CERTIFICATE_AUTHORITY, ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION,
    ACM_ACME_ENDPOINT, ACM_CERTIFICATE, AMPLIFYUIBUILDER_COMPONENT, AMPLIFYUIBUILDER_FORM,
    AMPLIFYUIBUILDER_THEME, AMPLIFY_APP, AOSS_COLLECTION,
    AOSS_SECURITY_POLICY, APIGATEWAY_BASE_PATH_MAPPING, APIGATEWAY_DOMAIN_NAME, APIGATEWAYV2_API,
    APIGATEWAYV2_API_MAPPING, APIGATEWAYV2_AUTHORIZER, APIGATEWAYV2_DEPLOYMENT,
    APIGATEWAYV2_DOMAIN_NAME, APIGATEWAYV2_INTEGRATION, APIGATEWAYV2_ROUTE,
    APIGATEWAYV2_STAGE, BACKUP_PLAN, BACKUP_SELECTION, BACKUP_VAULT, BATCH_COMPUTE_ENVIRONMENT,
    BATCH_JOB_QUEUE, BATCH_SERVICE_ENVIRONMENT, BEDROCK_GUARDRAIL, BEDROCK_KNOWLEDGE_BASE,
    BRAKET_SPENDING_LIMIT,
    CLOUDWATCH_DASHBOARD,
    CLOUDWATCH_METRIC_STREAM, CODEARTIFACT_DOMAIN, CODEARTIFACT_REPOSITORY,
    CODEBUILD_PROJECT,
    CODEBUILD_REPORT_GROUP, CODECOMMIT_REPOSITORY, CODEDEPLOY_APPLICATION,
    CODEDEPLOY_DEPLOYMENT_CONFIG, CODEDEPLOY_DEPLOYMENT_GROUP,
    AUTOSCALING_GROUP,
    COGNITO_USER_POOL_GROUP, DEFAULT_SCHEDULE_GROUP, EC2_LAUNCH_TEMPLATE, EC2_SECURITY_GROUP,
    EC2_SUBNET,
    EC2_VOLUME, EC2_VPC,
    ECR_PUBLIC_REPOSITORY, ECR_REPOSITORY, ECS_CLUSTER, ECS_SERVICE, EKS_ACCESS_ENTRY,
    EKS_CLUSTER,
    EKS_POD_IDENTITY_ASSOCIATION,
    EMR_CLUSTER, EVENTS_RULE, FIREHOSE_DELIVERY_STREAM, FIS_EXPERIMENT_TEMPLATE,
    ELASTIC_BEANSTALK_APPLICATION, ELASTIC_BEANSTALK_APPLICATION_VERSION,
    ELASTIC_BEANSTALK_ENVIRONMENT, GLUE_CRAWLER, GLUE_DATABASE, GLUE_TABLE, GRAFANA_WORKSPACE,
    GUARDDUTY_DETECTOR, GUARDDUTY_IP_SET,
    IAM_ACCESS_KEY,
    KINESIS_STREAM,
    KMS_ALIAS, KMS_KEY,
    MQ_BROKER, MSK_CLUSTER,
    MSK_SERVERLESS_CLUSTER, MWAA_ENVIRONMENT, OPENSEARCH_DOMAIN, PIPES_PIPE, REDSHIFT_CLUSTER,
    REDSHIFT_CLUSTER_SUBNET_GROUP, RESOURCE_GROUPS_GROUP, ROUTE53RESOLVER_ENDPOINT,
    ROUTE53RESOLVER_FIREWALL_DOMAIN_LIST, ROUTE53RESOLVER_FIREWALL_RULE_GROUP,
    ROUTE53RESOLVER_FIREWALL_RULE_GROUP_ASSOCIATION,
    ROUTE53RESOLVER_RULE,
    ROUTE53RESOLVER_RULE_ASSOCIATION, ROUTE53_HEALTH_CHECK, ROUTE53_HOSTED_ZONE,
    ROUTE53_CIDR_COLLECTION, ROUTE53_DELEGATION_SET, ROUTE53_TRAFFIC_POLICY,
    ROUTE53_RECORD_SET, ROUTE53_RECORD_SET_GROUP,
    S3TABLES_BUCKET_SEGMENT, S3TABLES_TABLE_BUCKET, S3TABLES_TABLE_BUCKET_ARN_ATTRIBUTE,
    S3VECTORS_BUCKET_SEGMENT, S3VECTORS_INDEX,
    S3VECTORS_VECTOR_BUCKET, SAGEMAKER_MODEL_PACKAGE_GROUP, SCHEDULER_SCHEDULE,
    SCHEDULER_SCHEDULE_GROUP, SES_CONFIGURATION_SET, SES_RECEIPT_RULE, SES_RECEIPT_RULE_SET,
    SNS_SUBSCRIPTION,
    SNS_TOPIC, SNS_TOPIC_INLINE_POLICY, SNS_TOPIC_POLICY, SQS_QUEUE, SQS_QUEUE_INLINE_POLICY,
    SQS_QUEUE_POLICY,
    SSM_ASSOCIATION, SSM_PARAMETER, STEP_FUNCTIONS_ACTIVITY,
    TRANSFER_SERVER, WAFV2_IP_SET, WAFV2_REGEX_PATTERN_SET, WAFV2_RULE_GROUP, WAFV2_WEB_ACL,
    WAFV2_WEB_ACL_ASSOCIATION, WAF_REGIONAL_IP_SET, XRAY_SAMPLING_RULE,
};
pub(super) use super::template::list_attribute_element;
use super::StoredResource;

/// `AWS::S3::Bucket`: "Ref returns the bucket name", so a resource of this type is a
/// bucket of Amazon S3 addressed by its physical resource id.
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// `AWS::IAM::Role`: "Ref returns the role name".
const IAM_ROLE: &str = "AWS::IAM::Role";

/// `AWS::IAM::InstanceProfile`: "Ref returns the instance profile name" — the handle every
/// IAM call on one takes (`GetInstanceProfile$InstanceProfileName`) and the name an
/// instance's `IamInstanceProfile` references, which is the physical id the naming rules
/// already compute for the type (its `InstanceProfileName` property), so the type needs no
/// entry in [`super::template::identity_is_service_assigned`].
const IAM_INSTANCE_PROFILE: &str = "AWS::IAM::InstanceProfile";

/// `AWS::IAM::User`: "Ref returns the user name", so a resource of this type is an IAM
/// user addressed by its physical resource id.
const IAM_USER: &str = "AWS::IAM::User";

/// `AWS::IAM::ManagedPolicy`: "Ref returns the ARN of the managed policy" — the handle
/// every IAM operation on a managed policy takes — which is why
/// [`super::template::identity_is_service_assigned`] names this type: the ARN carries the
/// account and the path IAM built it from, and it is what a template passes to another
/// resource's `ManagedPolicyArns` or `PermissionsBoundary`.
const IAM_MANAGED_POLICY: &str = "AWS::IAM::ManagedPolicy";

/// `AWS::CodePipeline::Pipeline`: "Ref returns the pipeline name".
const CODEPIPELINE_PIPELINE: &str = "AWS::CodePipeline::Pipeline";

/// `AWS::CodePipeline::CustomActionType`: the REGISTRATION of a custom action type in
/// CodePipeline, which `CreateCustomActionType` makes and `DeleteCustomActionType`
/// removes.
///
/// It is no named resource of that service's own: every call on one addresses it by the
/// triple its declaration states (`Category`, `Provider`, `Version`), which is what the
/// arms below read the registration's identity from — the physical resource id is not a
/// handle any CodePipeline operation takes.
const CODEPIPELINE_CUSTOM_ACTION_TYPE: &str = "AWS::CodePipeline::CustomActionType";

/// `AWS::RDS::DBInstance`: "Ref returns the DBInstanceIdentifier", so a resource of this
/// type is a DB instance of Amazon RDS addressed by its physical resource id.
const RDS_DB_INSTANCE: &str = "AWS::RDS::DBInstance";
/// `AWS::RDS::DBSubnetGroup`: "Ref returns the DBSubnetGroupName" — the group a DB
/// instance's `DBSubnetGroupName` names, which Amazon RDS resolves before it will create
/// an instance in it.
const RDS_DB_SUBNET_GROUP: &str = "AWS::RDS::DBSubnetGroup";

/// `AWS::RDS::DBParameterGroup`: "Ref returns the DB parameter group name" — the group a DB
/// instance's `DBParameterGroupName` names, and the one `DescribeDBParameters` reports the
/// parameters of.
const RDS_DB_PARAMETER_GROUP: &str = "AWS::RDS::DBParameterGroup";

/// `AWS::RDS::DBClusterParameterGroup`: "Ref returns the DB cluster parameter group name" —
/// the cluster-scoped sibling of the type above, which a DB cluster's
/// `DBClusterParameterGroupName` names.
const RDS_DB_CLUSTER_PARAMETER_GROUP: &str = "AWS::RDS::DBClusterParameterGroup";

/// `AWS::RDS::DBProxy`: "Ref returns the DBProxyName" — the identifier every Amazon RDS
/// call on a proxy takes, which is the physical id the naming rules already compute for the
/// type (its own `DBProxyName` property), so the type needs no entry in
/// [`super::template::identity_is_service_assigned`].
const RDS_DB_PROXY: &str = "AWS::RDS::DBProxy";

/// `AWS::RDS::DBProxyTargetGroup`: not a resource Amazon RDS creates on its own account —
/// a proxy comes up with the one target group it has, named `default` — but the
/// REGISTRATION of the databases that group routes to, plus the connection pool settings
/// of the group. Its identity is the `TargetGroupArn` that service minted, which is what
/// this type's `Ref` returns, so [`super::template::identity_is_service_assigned`] names
/// it.
const RDS_DB_PROXY_TARGET_GROUP: &str = "AWS::RDS::DBProxyTargetGroup";

/// `AWS::BedrockAgentCore::WorkloadIdentity`: "Ref returns the name" of the workload
/// identity — the handle every call of the Amazon Bedrock AgentCore control plane on one
/// takes (`GetWorkloadIdentity`, `UpdateWorkloadIdentity` and `DeleteWorkloadIdentity` each
/// address an identity by its `name`) — so a resource of this type is addressed by its
/// physical resource id.
const BEDROCK_AGENTCORE_WORKLOAD_IDENTITY: &str = "AWS::BedrockAgentCore::WorkloadIdentity";

/// `AWS::Glue::Job`: "Ref returns the job name" — the `Name` the declaration states, which
/// is what `CreateJob` creates the job under and what every AWS Glue operation on a job
/// definition (`GetJob`, `UpdateJob`, `StartJobRun`, `DeleteJob`) addresses it by. That is
/// the physical id the naming rules already compute for a type whose published return
/// values `super::template` carries no entry for, so the type needs none there.
const GLUE_JOB: &str = "AWS::Glue::Job";

/// The code a refusal carries where the change a deployment asks for is one this emulator
/// has not written the owning API call for. It is the same code the scaffold's own 501
/// answers under (`smithy.framework#NotImplemented`), and not a code any AWS service
/// answers, so it is never mistaken for one of theirs.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// How a resource provider built on the CloudFormation resource-provider plugin words a
/// refusal it classifies as that plugin's invalid-request failure: the class's own sentence
/// opener, with the owning service's message after it
/// ([`Provisioner::handler_message`]).
const INVALID_REQUEST_PROVIDED: &str = "Invalid request provided: ";

/// The services whose resources this emulator creates. One variant per resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Provisioner {
    FirehoseDeliveryStream,
    /// `AWS::Kinesis::Stream`: a data stream of Amazon Kinesis Data Streams — a different
    /// service from the Firehose one above — which `CreateStream` creates and
    /// `DeleteStream` deletes.
    KinesisStream,
    CloudWatchMetricStream,
    CloudWatchDashboard,
    /// `AWS::CloudWatch::Alarm`: the alarm Amazon CloudWatch's own `PutMetricAlarm` creates
    /// under the name the stack decided — the name this type's `Ref` returns and the handle
    /// every CloudWatch call about an alarm takes — and `DeleteAlarms` removes. That one
    /// call is documented "Creates or updates an alarm", so it is this type's update as
    /// well as its create ([`cloudwatch_alarms`]).
    CloudWatchAlarm,
    EcrPublicRepository,
    /// `AWS::ECR::Repository`: a repository of the PRIVATE Amazon ECR registry — a
    /// different service from the public one above — which `CreateRepository` creates
    /// under the repository name its declaration states and `DeleteRepository` deletes,
    /// with the resource-based policy its declaration asks for set on it.
    EcrRepository,
    /// `AWS::EC2::VPC`: a VPC of Amazon EC2, whose `vpc-…` identifier that service assigns.
    Ec2Vpc,
    /// `AWS::EC2::Subnet`: a subnet of one of those VPCs, whose `subnet-…` identifier is
    /// EC2's to assign the same way.
    Ec2Subnet,
    /// `AWS::EC2::SecurityGroup`: a security group of one of those VPCs, whose `sg-…`
    /// identifier is EC2's to assign the same way, with the inline rules its declaration
    /// asks for authorized on it.
    Ec2SecurityGroup,
    /// `AWS::EC2::Volume`: an Amazon EBS volume, whose `vol-…` identifier EC2 assigns the
    /// same way. `CreateVolume` answers the volume `creating`, so its creation is held open
    /// until that service reports it `available`.
    Ec2Volume,
    /// `AWS::EC2::LaunchTemplate`: a launch template of Amazon EC2, whose `lt-…` identifier
    /// that service assigns and whose version numbers it keeps —
    /// `CreateLaunchTemplate` creates it under the name the stack decided and
    /// `DeleteLaunchTemplate` deletes it by the id EC2 answered.
    Ec2LaunchTemplate,
    /// `AWS::AutoScaling::AutoScalingGroup`: a group of Amazon EC2 Auto Scaling, which
    /// `CreateAutoScalingGroup` creates under the name the stack decided — the name this
    /// type's `Ref` returns — and `DeleteAutoScalingGroup` deletes by that name, together
    /// with the instances the group launched.
    AutoScalingGroup,
    OpenSearchServerlessCollection,
    OpenSearchServerlessSecurityPolicy,
    /// `AWS::OpenSearchService::Domain`: a domain of Amazon OpenSearch Service, which is a
    /// different service from the serverless one above — `CreateDomain` creates it and
    /// `DeleteDomain` deletes it.
    OpenSearchServiceDomain,
    S3Bucket,
    /// `AWS::S3::BucketPolicy`: not a resource of Amazon S3's own but the POLICY of the
    /// bucket it names, which `PutBucketPolicy` writes and `DeleteBucketPolicy` removes —
    /// the queue-policy pair's rule ([`Provisioner::SqsQueuePolicy`]) for the service that
    /// holds a policy as a sub-resource of the bucket.
    S3BucketPolicy,
    /// `AWS::S3Tables::TableBucket`: the table bucket Amazon S3 Tables'
    /// `CreateTableBucket` creates under the name the template states and
    /// `DeleteTableBucket` removes. That service keys a table bucket by its own ARN, which
    /// is this type's physical id and what every call on one takes, so the arms address it
    /// by that ARN rather than by the bucket's name.
    S3TablesTableBucket,
    S3VectorsVectorBucket,
    S3VectorsIndex,
    EmrCluster,
    IamRole,
    /// `AWS::IAM::InstanceProfile`: the container that lets an Amazon EC2 instance assume
    /// the role it holds. `CreateInstanceProfile` creates it and
    /// `AddRoleToInstanceProfile` puts the declared role in it.
    IamInstanceProfile,
    IamUser,
    /// `AWS::IAM::AccessKey`: the long-term credential of one of those users, which
    /// `CreateAccessKey` mints and `DeleteAccessKey` deletes. Its id and its secret are
    /// IAM's to generate, so both are read off that create's answer.
    IamAccessKey,
    IamManagedPolicy,
    CodePipelinePipeline,
    /// `AWS::CodePipeline::CustomActionType`: the custom action type a job worker of the
    /// account polls for, which `CreateCustomActionType` registers from the declaration's
    /// own members and `DeleteCustomActionType` removes by the triple that identifies it.
    CodePipelineCustomActionType,
    /// `AWS::CodeBuild::ReportGroup`: a report group of AWS CodeBuild, which
    /// `CreateReportGroup` creates under the name the template states and
    /// `DeleteReportGroup` removes by the ARN that service keyed it by.
    CodeBuildReportGroup,
    /// `AWS::CodeBuild::Project`: a build project of that same service, which
    /// `CreateProject` creates under the name the template states and `DeleteProject`
    /// removes by that name — so the project a stack reports created is one every
    /// CodeBuild read and every `StartBuild` resolves.
    CodeBuildProject,
    EcsCluster,
    /// `AWS::ECS::Service`: the service Amazon ECS's own `CreateService` creates on the
    /// cluster the declaration names, `UpdateService` changes and `DeleteService` removes.
    /// "Ref returns the Amazon Resource Name (ARN)" of the service, which that create
    /// answers and every call about the service takes ([`ecs_services`]).
    EcsService,
    SqsQueue,
    /// `AWS::SQS::QueuePolicy` and `AWS::SQS::QueueInlinePolicy`, which are not resources
    /// of Amazon SQS's own: each is the `Policy` ATTRIBUTE of the queues it names, so both
    /// are deployed through `SetQueueAttributes` and differ only in which property carries
    /// the queue.
    SqsQueuePolicy,
    SageMakerModelPackageGroup,
    /// `AWS::SageMaker::Model`: the inference model `CreateModel` registers under the name
    /// the template decided — the name every SageMaker call on a model takes — and
    /// `DeleteModel` removes.
    SageMakerModel,
    /// `AWS::SageMaker::NotebookInstance`: the managed Jupyter instance
    /// `CreateNotebookInstance` launches under that name. That create answers the instance
    /// `Pending`, in which it serves no session, so the resource's creation is held open
    /// until SageMaker reports it `InService`, and its deletion stops the instance before
    /// `DeleteNotebookInstance` can take it away.
    SageMakerNotebookInstance,
    /// `AWS::SageMaker::NotebookInstanceLifecycleConfig`: the `OnCreate`/`OnStart` scripts a
    /// notebook instance's `LifecycleConfigName` names, which
    /// `CreateNotebookInstanceLifecycleConfig` creates and
    /// `DeleteNotebookInstanceLifecycleConfig` removes.
    SageMakerNotebookInstanceLifecycleConfig,
    SnsTopic,
    /// `AWS::SNS::TopicPolicy` and `AWS::SNS::TopicInlinePolicy`, which are not resources
    /// of Amazon SNS's own: each is the `Policy` ATTRIBUTE of the topics it names, so both
    /// are deployed through `SetTopicAttributes` and differ only in which property carries
    /// the topic — the queue-policy pair's rule ([`Provisioner::SqsQueuePolicy`]) for the
    /// other service that holds a policy as an attribute.
    SnsTopicPolicy,
    /// `AWS::SNS::Subscription`: a subscriber of a topic, which is a resource of Amazon
    /// SNS's own — `Subscribe` creates it and `Unsubscribe` deletes it — beside the inline
    /// `Subscription` list an `AWS::SNS::Topic` may declare.
    SnsSubscription,
    FisExperimentTemplate,
    WafRegionalIpSet,
    Wafv2WebAcl,
    Wafv2RuleGroup,
    Wafv2IpSet,
    Wafv2RegexPatternSet,
    /// `AWS::WAFv2::WebACLAssociation`: not an entity of AWS WAF's own but the BINDING that
    /// puts one of its web ACLs in front of a protected resource, which `AssociateWebACL`
    /// makes and `DisassociateWebACL` removes.
    Wafv2WebAclAssociation,
    GlueJob,
    /// `AWS::Glue::Database`: a database of the AWS Glue Data Catalog, which
    /// `CreateDatabase` creates under the name its `DatabaseInput` states and
    /// `DeleteDatabase` deletes.
    GlueDatabase,
    /// `AWS::Glue::Table`: a table IN one of those databases, which `CreateTable` creates
    /// under the name its `TableInput` states and `DeleteTable` deletes — addressed by the
    /// PAIR that service keys a table by, its own name and the database that holds it.
    GlueTable,
    /// `AWS::Glue::Crawler`: a crawler of that same catalog, which `CreateCrawler` creates
    /// under the name the declaration states and `DeleteCrawler` deletes.
    GlueCrawler,
    GrafanaWorkspace,
    GuardDutyDetector,
    /// `AWS::GuardDuty::IPSet`: a trusted IP list of one of those detectors, which
    /// `CreateIPSet` creates in the detector its declaration names and `DeleteIPSet`
    /// deletes. GuardDuty mints the `IpSetId` that type's `Ref` returns.
    GuardDutyIpSet,
    KmsKey,
    KmsAlias,
    MskCluster,
    MskServerlessCluster,
    MqBroker,
    RdsDbInstance,
    /// `AWS::ElastiCache::CacheCluster`: a cache cluster of Amazon ElastiCache, which
    /// `CreateCacheCluster` creates under the `ClusterName` the declaration states — the name
    /// this type's `Ref` returns — and `DeleteCacheCluster` deletes by it. That create
    /// answers the cluster `creating`, in which it serves nothing and carries no endpoint, so
    /// the resource's creation is held open until ElastiCache reports it `available`.
    ElastiCacheCacheCluster,
    RdsDbSubnetGroup,
    RdsDbParameterGroup,
    RdsDbClusterParameterGroup,
    /// `AWS::RDS::DBProxy`: an RDS proxy, which `CreateDBProxy` creates under the name the
    /// template states and `DeleteDBProxy` deletes. The create answers the proxy
    /// `creating`, so its creation is held open until Amazon RDS reports it `available`.
    RdsDbProxy,
    /// `AWS::RDS::DBProxyTargetGroup`: the databases one of those proxies routes to, which
    /// `RegisterDBProxyTargets` registers in the proxy's own target group and
    /// `DeregisterDBProxyTargets` removes, with the connection pool settings the
    /// declaration asks for applied by `ModifyDBProxyTargetGroup`.
    RdsDbProxyTargetGroup,
    RedshiftCluster,
    /// `AWS::Redshift::ClusterSubnetGroup`: the subnet group a provisioned Redshift cluster
    /// is launched into, which `CreateClusterSubnetGroup` creates under the name
    /// CloudFormation generated for the resource and `DeleteClusterSubnetGroup` deletes.
    ///
    /// Amazon Redshift resolves a cluster's `ClusterSubnetGroupName` against the groups it
    /// HOLDS before it will create a cluster in one (`ClusterSubnetGroupNotFoundFault`), so
    /// a stack declaring both has to create this one for real too — exactly the position
    /// `AWS::RDS::DBSubnetGroup` is in ([`Provisioner::RdsDbSubnetGroup`]).
    RedshiftClusterSubnetGroup,
    /// `AWS::Redshift::ClusterParameterGroup`: the parameter group a provisioned Redshift
    /// cluster's `ClusterParameterGroupName` names, which `CreateClusterParameterGroup`
    /// creates under the name the declaration states — the name this type's `Ref` returns —
    /// `ModifyClusterParameterGroup` writes the declared settings of and
    /// `DeleteClusterParameterGroup` deletes.
    ///
    /// Amazon Redshift resolves a cluster's `ClusterParameterGroupName` against the groups it
    /// HOLDS before it will create a cluster configured from one
    /// (`ClusterParameterGroupNotFoundFault`), so a stack declaring both has to create this
    /// one for real too — exactly the position [`Provisioner::RedshiftClusterSubnetGroup`] is
    /// in.
    RedshiftClusterParameterGroup,
    EventsRule,
    /// `AWS::Pipes::Pipe`: a pipe of Amazon EventBridge Pipes, which `CreatePipe` creates
    /// under the name the template states, `UpdatePipe` changes and `DeletePipe` deletes.
    /// That create answers the pipe `CREATING`, in which it moves no record, so the
    /// resource's creation is held open until that service reports it resting in the state
    /// its `DesiredState` asked for.
    PipesPipe,
    SsmParameter,
    /// `AWS::ResourceGroups::Group`: a resource group of Resource Groups, which
    /// `CreateGroup` creates under the name the template states — the name this type's `Ref`
    /// returns and the identifier every Resource Groups call on one takes — `UpdateGroup` and
    /// `UpdateGroupQuery` change and `DeleteGroup` removes. The ARN the type publishes is the
    /// one that service mints, under its own hyphenated `arnNamespace`, so it is read off the
    /// create's own answer.
    ResourceGroupsGroup,
    /// `AWS::SSM::Association`: a State Manager association of AWS Systems Manager, which
    /// `CreateAssociation` creates, `UpdateAssociation` changes and `DeleteAssociation`
    /// removes.
    ///
    /// Its identity is the `AssociationId` that service MINTS: the create takes no identifier
    /// for the association at all — the `Name` a declaration states names the DOCUMENT the
    /// association applies — and every call on one takes that id, which is why
    /// `template::identity_is_service_assigned` names the type and the create replaces the
    /// stand-in the naming rules computed with the id Systems Manager answered.
    SsmAssociation,
    StepFunctionsActivity,
    SchedulerScheduleGroup,
    SchedulerSchedule,
    CognitoUserPoolGroup,
    AcmAcmeEndpoint,
    AcmCertificate,
    /// `AWS::ACMPCA::CertificateAuthority`: a private certificate authority of Amazon Web
    /// Services Private CA, whose ARN that service mints — `CreateCertificateAuthority`
    /// creates it and `DeleteCertificateAuthority` deletes it.
    AcmPcaCertificateAuthority,
    /// `AWS::ACMPCA::Certificate`: a certificate one of those CAs ISSUED, which
    /// `IssueCertificate` creates and whose ARN that service mints. Amazon Web Services
    /// Private CA publishes no call that un-issues one, so the type's deployment has
    /// nothing to delete.
    AcmPcaCertificate,
    /// `AWS::ACMPCA::CertificateAuthorityActivation`: not a resource of that service's own
    /// but the ACTIVATION of one of its CAs — `ImportCertificateAuthorityCertificate` puts
    /// the issued certificate into the CA, and `UpdateCertificateAuthority` applies the
    /// status the declaration asks for.
    AcmPcaCertificateAuthorityActivation,
    Route53ResolverEndpoint,
    Route53ResolverRule,
    Route53ResolverRuleAssociation,
    Route53ResolverFirewallDomainList,
    Route53ResolverFirewallRuleGroup,
    Route53ResolverFirewallRuleGroupAssociation,
    Route53HostedZone,
    Route53RecordSet,
    Route53RecordSetGroup,
    Route53HealthCheck,
    Route53CidrCollection,
    Route53DelegationSet,
    Route53TrafficPolicy,
    ElasticBeanstalkApplication,
    ElasticBeanstalkApplicationVersion,
    ElasticBeanstalkEnvironment,
    TransferServer,
    CodeDeployApplication,
    CodeDeployDeploymentGroup,
    CodeDeployDeploymentConfig,
    BatchComputeEnvironment,
    BatchJobQueue,
    BatchServiceEnvironment,
    EksAccessEntry,
    /// `AWS::EKS::Cluster`: the Kubernetes control plane `CreateCluster` creates under the
    /// name the template states and `DeleteCluster` removes. That create is asynchronous,
    /// so the resource's creation is held open until Amazon EKS reports the cluster
    /// `ACTIVE`.
    EksCluster,
    /// `AWS::EKS::PodIdentityAssociation`: the IAM role one Kubernetes service account
    /// assumes, which `CreatePodIdentityAssociation` makes and
    /// `DeletePodIdentityAssociation` removes.
    EksPodIdentityAssociation,
    /// `AWS::ApiGateway::DomainName`: a custom domain name of the REST control plane, which
    /// `CreateDomainName` creates under the name the template states and
    /// `DeleteDomainName` removes.
    ApiGatewayDomainName,
    /// `AWS::ApiGateway::BasePathMapping`: the base path of a custom domain name that a
    /// stage of a REST API answers on, which `CreateBasePathMapping` creates under that
    /// domain name and `DeleteBasePathMapping` removes.
    ApiGatewayBasePathMapping,
    ApiGatewayV2Api,
    ApiGatewayV2DomainName,
    ApiGatewayV2ApiMapping,
    ApiGatewayV2Integration,
    ApiGatewayV2Route,
    ApiGatewayV2Stage,
    ApiGatewayV2Deployment,
    ApiGatewayV2Authorizer,
    CodeCommitRepository,
    /// `AWS::CodeArtifact::Domain`: a CodeArtifact domain, which `CreateDomain` creates
    /// under the name the template states and `DeleteDomain` removes.
    CodeArtifactDomain,
    /// `AWS::CodeArtifact::Repository`: a repository IN such a domain, which
    /// `CreateRepository` creates there and `DeleteRepository` removes — with every external
    /// connection its declaration asks for associated, which is a call of its own.
    CodeArtifactRepository,
    SesConfigurationSet,
    /// `AWS::SES::ReceiptRuleSet`: a receipt rule set of Amazon SES, which
    /// `CreateReceiptRuleSet` creates empty and `DeleteReceiptRuleSet` deletes.
    SesReceiptRuleSet,
    /// `AWS::SES::ReceiptRule`: one rule OF such a set, which `CreateReceiptRule` adds to
    /// the set its declaration names and `DeleteReceiptRule` removes from it.
    SesReceiptRule,
    BackupVault,
    BackupPlan,
    BackupSelection,
    /// `AWS::Bedrock::Guardrail`: a guardrail of Amazon Bedrock's own control plane, which
    /// `CreateGuardrail` creates under the name the template states, `UpdateGuardrail`
    /// changes and `DeleteGuardrail` removes. Amazon Bedrock MINTS the `guardrailId` and
    /// the ARN the type publishes, so both are read off that create's own answer and the
    /// ARN is the identity a `Ref` resolves to.
    BedrockGuardrail,
    /// `AWS::Bedrock::KnowledgeBase`: a knowledge base of Amazon Bedrock Agent's build-time
    /// control plane, which `CreateKnowledgeBase` creates under the name the template states,
    /// `UpdateKnowledgeBase` changes and `DeleteKnowledgeBase` removes. That service MINTS
    /// the ten-character `knowledgeBaseId` the type publishes — the handle every
    /// knowledge-base API takes — so it is read off that create's own answer and is the
    /// identity a `Ref` resolves to.
    BedrockKnowledgeBase,
    /// `AWS::BedrockAgentCore::WorkloadIdentity`: a workload identity of the Amazon Bedrock
    /// AgentCore control plane, which `CreateWorkloadIdentity` creates under the name the
    /// template states, `UpdateWorkloadIdentity` changes and `DeleteWorkloadIdentity`
    /// removes. The name IS the identity's handle — every call on one takes it — so it is
    /// the physical id the naming rules already compute for the type.
    BedrockAgentCoreWorkloadIdentity,
    /// `AWS::Braket::SpendingLimit`: a spending limit of Amazon Braket's own control plane,
    /// which `CreateSpendingLimit` makes for the quantum device the template names,
    /// `UpdateSpendingLimit` changes and `DeleteSpendingLimit` removes. That service MINTS
    /// the `spendingLimitArn` the type publishes — the handle every spending-limit API takes
    /// — so it is read off that create's own answer and is the identity a `Ref` resolves to.
    BraketSpendingLimit,
    /// `AWS::AmplifyUIBuilder::Component`, `::Form` and `::Theme`: the three user-interface
    /// entities of an Amplify app environment, which Amplify UI Builder's own
    /// `CreateComponent`, `CreateForm` and `CreateTheme` make under the name the template
    /// states, the matching `Update…` changes and the matching `Delete…` removes. That service
    /// MINTS the `id` each type publishes — the `Uuid` every call on the entity is addressed by,
    /// beside the app and the environment its URI carries — so it is read off that create's own
    /// answer and is the identity a `Ref` resolves to. One variant per type, each dispatching to
    /// the arms of [`amplify_ui_builder`] with the entity it names.
    AmplifyUiBuilderComponent,
    AmplifyUiBuilderForm,
    AmplifyUiBuilderTheme,
    /// `AWS::Amplify::App`: the hosting application AWS Amplify's own `CreateApp` makes under
    /// the name the template states, `UpdateApp` changes and `DeleteApp` removes — the parent
    /// every branch, domain and Amplify UI Builder entity of an app hangs off. That service
    /// MINTS the `appId` and the `appArn` the type publishes, so both are read off that
    /// create's own answer and the ARN is the identity a `Ref` resolves to.
    AmplifyApp,
    /// `AWS::MWAA::Environment`: the Apache Airflow environment Amazon Managed Workflows for
    /// Apache Airflow's `CreateEnvironment` creates under the name the template states,
    /// `UpdateEnvironment` reconfigures and `DeleteEnvironment` removes. The name IS the
    /// handle every operation on one takes, so it is the physical id the naming rules already
    /// compute for the type; both of those lifecycle changes are asynchronous, so the
    /// resource's creation and its deletion are each held open on that service's own report
    /// of the environment.
    MwaaEnvironment,
    /// `AWS::XRay::SamplingRule`: a sampling rule of AWS X-Ray, which
    /// `CreateSamplingRule` creates under the rule name its declaration states and
    /// `DeleteSamplingRule` deletes. X-Ray mints the `RuleARN` that type's `Ref`
    /// returns and every call on a rule can address it by.
    XraySamplingRule,
    /// `AWS::Config::ConfigurationRecorder`: the configuration recorder of AWS Config,
    /// which `PutConfigurationRecorder` creates under the name the template states and
    /// `DeleteConfigurationRecorder` deletes.
    ConfigConfigurationRecorder,
    /// `AWS::Config::DeliveryChannel`: the channel that recorder delivers through, which
    /// `PutDeliveryChannel` creates and `DeleteDeliveryChannel` deletes.
    ConfigDeliveryChannel,
    /// `AWS::Config::ConfigRule`: a Config rule, which `PutConfigRule` creates under the
    /// rule name the template states and `DeleteConfigRule` deletes. Config mints the ARN
    /// and the rule id the type publishes, so both are read off that service's own read of
    /// the rule.
    ConfigConfigRule,
}

/// The provisioner that owns a template resource type, or `None` for a type this emulator
/// leaves declared-only.
pub(super) fn provisioner_for(resource_type: &str) -> Option<Provisioner> {
    match resource_type {
        FIREHOSE_DELIVERY_STREAM => Some(Provisioner::FirehoseDeliveryStream),
        KINESIS_STREAM => Some(Provisioner::KinesisStream),
        CLOUDWATCH_METRIC_STREAM => Some(Provisioner::CloudWatchMetricStream),
        cloudwatch_alarms::CLOUDWATCH_ALARM => Some(Provisioner::CloudWatchAlarm),
        CLOUDWATCH_DASHBOARD => Some(Provisioner::CloudWatchDashboard),
        ECR_PUBLIC_REPOSITORY => Some(Provisioner::EcrPublicRepository),
        ECR_REPOSITORY => Some(Provisioner::EcrRepository),
        EC2_VPC => Some(Provisioner::Ec2Vpc),
        EC2_SUBNET => Some(Provisioner::Ec2Subnet),
        EC2_SECURITY_GROUP => Some(Provisioner::Ec2SecurityGroup),
        EC2_VOLUME => Some(Provisioner::Ec2Volume),
        EC2_LAUNCH_TEMPLATE => Some(Provisioner::Ec2LaunchTemplate),
        AUTOSCALING_GROUP => Some(Provisioner::AutoScalingGroup),
        AOSS_COLLECTION => Some(Provisioner::OpenSearchServerlessCollection),
        AOSS_SECURITY_POLICY => Some(Provisioner::OpenSearchServerlessSecurityPolicy),
        OPENSEARCH_DOMAIN => Some(Provisioner::OpenSearchServiceDomain),
        S3_BUCKET => Some(Provisioner::S3Bucket),
        S3_BUCKET_POLICY => Some(Provisioner::S3BucketPolicy),
        S3TABLES_TABLE_BUCKET => Some(Provisioner::S3TablesTableBucket),
        S3VECTORS_VECTOR_BUCKET => Some(Provisioner::S3VectorsVectorBucket),
        S3VECTORS_INDEX => Some(Provisioner::S3VectorsIndex),
        EMR_CLUSTER => Some(Provisioner::EmrCluster),
        IAM_ROLE => Some(Provisioner::IamRole),
        IAM_INSTANCE_PROFILE => Some(Provisioner::IamInstanceProfile),
        IAM_USER => Some(Provisioner::IamUser),
        IAM_ACCESS_KEY => Some(Provisioner::IamAccessKey),
        IAM_MANAGED_POLICY => Some(Provisioner::IamManagedPolicy),
        CODEPIPELINE_PIPELINE => Some(Provisioner::CodePipelinePipeline),
        CODEPIPELINE_CUSTOM_ACTION_TYPE => Some(Provisioner::CodePipelineCustomActionType),
        CODEBUILD_REPORT_GROUP => Some(Provisioner::CodeBuildReportGroup),
        CODEBUILD_PROJECT => Some(Provisioner::CodeBuildProject),
        ECS_CLUSTER => Some(Provisioner::EcsCluster),
        ECS_SERVICE => Some(Provisioner::EcsService),
        SQS_QUEUE => Some(Provisioner::SqsQueue),
        SQS_QUEUE_POLICY | SQS_QUEUE_INLINE_POLICY => Some(Provisioner::SqsQueuePolicy),
        SAGEMAKER_MODEL_PACKAGE_GROUP => Some(Provisioner::SageMakerModelPackageGroup),
        SAGEMAKER_MODEL => Some(Provisioner::SageMakerModel),
        SAGEMAKER_NOTEBOOK_INSTANCE => Some(Provisioner::SageMakerNotebookInstance),
        SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG => {
            Some(Provisioner::SageMakerNotebookInstanceLifecycleConfig)
        }
        SNS_TOPIC => Some(Provisioner::SnsTopic),
        SNS_TOPIC_POLICY | SNS_TOPIC_INLINE_POLICY => Some(Provisioner::SnsTopicPolicy),
        SNS_SUBSCRIPTION => Some(Provisioner::SnsSubscription),
        FIS_EXPERIMENT_TEMPLATE => Some(Provisioner::FisExperimentTemplate),
        WAF_REGIONAL_IP_SET => Some(Provisioner::WafRegionalIpSet),
        WAFV2_WEB_ACL => Some(Provisioner::Wafv2WebAcl),
        WAFV2_RULE_GROUP => Some(Provisioner::Wafv2RuleGroup),
        WAFV2_IP_SET => Some(Provisioner::Wafv2IpSet),
        WAFV2_REGEX_PATTERN_SET => Some(Provisioner::Wafv2RegexPatternSet),
        WAFV2_WEB_ACL_ASSOCIATION => Some(Provisioner::Wafv2WebAclAssociation),
        GLUE_JOB => Some(Provisioner::GlueJob),
        GLUE_DATABASE => Some(Provisioner::GlueDatabase),
        GLUE_TABLE => Some(Provisioner::GlueTable),
        GLUE_CRAWLER => Some(Provisioner::GlueCrawler),
        GRAFANA_WORKSPACE => Some(Provisioner::GrafanaWorkspace),
        GUARDDUTY_DETECTOR => Some(Provisioner::GuardDutyDetector),
        GUARDDUTY_IP_SET => Some(Provisioner::GuardDutyIpSet),
        KMS_KEY => Some(Provisioner::KmsKey),
        KMS_ALIAS => Some(Provisioner::KmsAlias),
        MSK_CLUSTER => Some(Provisioner::MskCluster),
        MSK_SERVERLESS_CLUSTER => Some(Provisioner::MskServerlessCluster),
        MQ_BROKER => Some(Provisioner::MqBroker),
        RDS_DB_INSTANCE => Some(Provisioner::RdsDbInstance),
        ELASTICACHE_CACHE_CLUSTER => Some(Provisioner::ElastiCacheCacheCluster),
        RDS_DB_SUBNET_GROUP => Some(Provisioner::RdsDbSubnetGroup),
        RDS_DB_PARAMETER_GROUP => Some(Provisioner::RdsDbParameterGroup),
        RDS_DB_CLUSTER_PARAMETER_GROUP => Some(Provisioner::RdsDbClusterParameterGroup),
        RDS_DB_PROXY => Some(Provisioner::RdsDbProxy),
        RDS_DB_PROXY_TARGET_GROUP => Some(Provisioner::RdsDbProxyTargetGroup),
        REDSHIFT_CLUSTER => Some(Provisioner::RedshiftCluster),
        REDSHIFT_CLUSTER_SUBNET_GROUP => Some(Provisioner::RedshiftClusterSubnetGroup),
        REDSHIFT_CLUSTER_PARAMETER_GROUP => Some(Provisioner::RedshiftClusterParameterGroup),
        EVENTS_RULE => Some(Provisioner::EventsRule),
        PIPES_PIPE => Some(Provisioner::PipesPipe),
        RESOURCE_GROUPS_GROUP => Some(Provisioner::ResourceGroupsGroup),
        SSM_PARAMETER => Some(Provisioner::SsmParameter),
        SSM_ASSOCIATION => Some(Provisioner::SsmAssociation),
        STEP_FUNCTIONS_ACTIVITY => Some(Provisioner::StepFunctionsActivity),
        SCHEDULER_SCHEDULE_GROUP => Some(Provisioner::SchedulerScheduleGroup),
        SCHEDULER_SCHEDULE => Some(Provisioner::SchedulerSchedule),
        COGNITO_USER_POOL_GROUP => Some(Provisioner::CognitoUserPoolGroup),
        ACM_ACME_ENDPOINT => Some(Provisioner::AcmAcmeEndpoint),
        ACM_CERTIFICATE => Some(Provisioner::AcmCertificate),
        ACMPCA_CERTIFICATE_AUTHORITY => Some(Provisioner::AcmPcaCertificateAuthority),
        ACMPCA_CERTIFICATE => Some(Provisioner::AcmPcaCertificate),
        ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION => {
            Some(Provisioner::AcmPcaCertificateAuthorityActivation)
        }
        ROUTE53RESOLVER_ENDPOINT => Some(Provisioner::Route53ResolverEndpoint),
        ROUTE53RESOLVER_RULE => Some(Provisioner::Route53ResolverRule),
        ROUTE53RESOLVER_RULE_ASSOCIATION => Some(Provisioner::Route53ResolverRuleAssociation),
        ROUTE53RESOLVER_FIREWALL_DOMAIN_LIST => {
            Some(Provisioner::Route53ResolverFirewallDomainList)
        }
        ROUTE53RESOLVER_FIREWALL_RULE_GROUP => Some(Provisioner::Route53ResolverFirewallRuleGroup),
        ROUTE53RESOLVER_FIREWALL_RULE_GROUP_ASSOCIATION => {
            Some(Provisioner::Route53ResolverFirewallRuleGroupAssociation)
        }
        ROUTE53_HOSTED_ZONE => Some(Provisioner::Route53HostedZone),
        ROUTE53_RECORD_SET => Some(Provisioner::Route53RecordSet),
        ROUTE53_RECORD_SET_GROUP => Some(Provisioner::Route53RecordSetGroup),
        ROUTE53_HEALTH_CHECK => Some(Provisioner::Route53HealthCheck),
        ROUTE53_CIDR_COLLECTION => Some(Provisioner::Route53CidrCollection),
        ROUTE53_DELEGATION_SET => Some(Provisioner::Route53DelegationSet),
        ROUTE53_TRAFFIC_POLICY => Some(Provisioner::Route53TrafficPolicy),
        ELASTIC_BEANSTALK_APPLICATION => Some(Provisioner::ElasticBeanstalkApplication),
        ELASTIC_BEANSTALK_APPLICATION_VERSION => {
            Some(Provisioner::ElasticBeanstalkApplicationVersion)
        }
        ELASTIC_BEANSTALK_ENVIRONMENT => Some(Provisioner::ElasticBeanstalkEnvironment),
        TRANSFER_SERVER => Some(Provisioner::TransferServer),
        CODEDEPLOY_APPLICATION => Some(Provisioner::CodeDeployApplication),
        CODEDEPLOY_DEPLOYMENT_GROUP => Some(Provisioner::CodeDeployDeploymentGroup),
        CODEDEPLOY_DEPLOYMENT_CONFIG => Some(Provisioner::CodeDeployDeploymentConfig),
        BATCH_COMPUTE_ENVIRONMENT => Some(Provisioner::BatchComputeEnvironment),
        BATCH_JOB_QUEUE => Some(Provisioner::BatchJobQueue),
        BATCH_SERVICE_ENVIRONMENT => Some(Provisioner::BatchServiceEnvironment),
        EKS_ACCESS_ENTRY => Some(Provisioner::EksAccessEntry),
        EKS_CLUSTER => Some(Provisioner::EksCluster),
        EKS_POD_IDENTITY_ASSOCIATION => Some(Provisioner::EksPodIdentityAssociation),
        APIGATEWAY_DOMAIN_NAME => Some(Provisioner::ApiGatewayDomainName),
        APIGATEWAY_BASE_PATH_MAPPING => Some(Provisioner::ApiGatewayBasePathMapping),
        APIGATEWAYV2_API => Some(Provisioner::ApiGatewayV2Api),
        APIGATEWAYV2_DOMAIN_NAME => Some(Provisioner::ApiGatewayV2DomainName),
        APIGATEWAYV2_API_MAPPING => Some(Provisioner::ApiGatewayV2ApiMapping),
        APIGATEWAYV2_INTEGRATION => Some(Provisioner::ApiGatewayV2Integration),
        APIGATEWAYV2_ROUTE => Some(Provisioner::ApiGatewayV2Route),
        APIGATEWAYV2_STAGE => Some(Provisioner::ApiGatewayV2Stage),
        APIGATEWAYV2_DEPLOYMENT => Some(Provisioner::ApiGatewayV2Deployment),
        APIGATEWAYV2_AUTHORIZER => Some(Provisioner::ApiGatewayV2Authorizer),
        CODECOMMIT_REPOSITORY => Some(Provisioner::CodeCommitRepository),
        CODEARTIFACT_DOMAIN => Some(Provisioner::CodeArtifactDomain),
        CODEARTIFACT_REPOSITORY => Some(Provisioner::CodeArtifactRepository),
        SES_CONFIGURATION_SET => Some(Provisioner::SesConfigurationSet),
        SES_RECEIPT_RULE_SET => Some(Provisioner::SesReceiptRuleSet),
        SES_RECEIPT_RULE => Some(Provisioner::SesReceiptRule),
        BACKUP_VAULT => Some(Provisioner::BackupVault),
        BACKUP_PLAN => Some(Provisioner::BackupPlan),
        BACKUP_SELECTION => Some(Provisioner::BackupSelection),
        BEDROCK_GUARDRAIL => Some(Provisioner::BedrockGuardrail),
        BEDROCK_KNOWLEDGE_BASE => Some(Provisioner::BedrockKnowledgeBase),
        BEDROCK_AGENTCORE_WORKLOAD_IDENTITY => {
            Some(Provisioner::BedrockAgentCoreWorkloadIdentity)
        }
        BRAKET_SPENDING_LIMIT => Some(Provisioner::BraketSpendingLimit),
        AMPLIFYUIBUILDER_COMPONENT => Some(Provisioner::AmplifyUiBuilderComponent),
        AMPLIFYUIBUILDER_FORM => Some(Provisioner::AmplifyUiBuilderForm),
        AMPLIFYUIBUILDER_THEME => Some(Provisioner::AmplifyUiBuilderTheme),
        AMPLIFY_APP => Some(Provisioner::AmplifyApp),
        MWAA_ENVIRONMENT => Some(Provisioner::MwaaEnvironment),
        XRAY_SAMPLING_RULE => Some(Provisioner::XraySamplingRule),
        CONFIG_CONFIGURATION_RECORDER => Some(Provisioner::ConfigConfigurationRecorder),
        CONFIG_DELIVERY_CHANNEL => Some(Provisioner::ConfigDeliveryChannel),
        CONFIG_CONFIG_RULE => Some(Provisioner::ConfigConfigRule),
        _ => None,
    }
}

impl Provisioner {
    /// Create one resource in the service that owns it, and report back what that service
    /// decided about it ([`Created`]): the attributes it reported (an ARN it minted, say),
    /// which replace the ones CloudFormation's naming rules computed, and — for a type
    /// whose identity is that service's
    /// ([`super::template::identity_is_service_assigned`]) — the physical id it assigned.
    /// `Err` is that service's own refusal.
    ///
    /// `propagated` are the tags CloudFormation gives the resource — the stack's own plus
    /// the three that name the stack and this resource's place in it
    /// ([`super::provisioning::StackProvenance::tags_for`]) — and every arm whose type is
    /// taggable applies them ([`tags_of`]) beside the ones the declaration itself asks for,
    /// so the owning service's tag read on the handle the stack publishes answers with
    /// them. The one arm that applies none is `AWS::WAFRegional::IPSet`, whose type
    /// publishes no `Tags` property at all: CloudFormation propagates no tags to a resource
    /// type that is not taggable.
    pub(super) async fn create(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        propagated: &[(String, String)],
    ) -> Result<Created, Refusal> {
        let propagated = &self.applicable_tags(propagated);
        match self {
            Self::FirehoseDeliveryStream => {
                create_delivery_stream(ctx, region, resource, propagated).await
            }
            Self::KinesisStream => {
                create_kinesis_stream(ctx, region, resource, propagated).await
            }
            Self::CloudWatchMetricStream => {
                put_metric_stream(ctx, region, resource, propagated).await
            }
            Self::CloudWatchDashboard => {
                put_dashboard(ctx, region, resource, propagated).await
            }
            // `AWS::CloudWatch::Alarm` publishes no `Tags` property at all — CloudWatch holds
            // an alarm's tags through its own tagging calls and `PutMetricAlarm` takes a
            // `Tags` member the type does not declare — so no tags are propagated to one.
            Self::CloudWatchAlarm => put_metric_alarm(ctx, region, resource, propagated).await,
            Self::EmrCluster => run_job_flow(ctx, region, resource, propagated).await,
            Self::EcrPublicRepository => {
                create_public_repository(ctx, region, resource, propagated).await
            }
            Self::EcrRepository => create_repository(ctx, region, resource, propagated).await,
            Self::Ec2Vpc => create_ec2_vpc(ctx, region, resource, propagated).await,
            Self::Ec2Subnet => create_ec2_subnet(ctx, region, resource, propagated).await,
            Self::Ec2SecurityGroup => {
                create_ec2_security_group(ctx, region, resource, propagated).await
            }
            Self::Ec2Volume => create_ec2_volume(ctx, region, resource, propagated).await,
            Self::Ec2LaunchTemplate => {
                create_ec2_launch_template(ctx, region, resource, propagated).await
            }
            Self::AutoScalingGroup => {
                create_auto_scaling_group(ctx, region, resource, propagated).await
            }
            // Boxed, as the deeper arms of this match are: a bucket's create sends the
            // declared access controls and notification configuration after the create,
            // so this arm's own future is one of the largest here and a stack it shares
            // with the provisioning walk above it is the scarce resource.
            Self::S3Bucket => Box::pin(create_bucket(ctx, region, resource, propagated)).await,
            // `AWS::S3::BucketPolicy` publishes `Bucket` and `PolicyDocument` and no
            // `Tags`: a bucket's policy is no taggable resource, so no tags are
            // propagated to one.
            Self::S3BucketPolicy => put_bucket_policy(ctx, region, resource).await,
            // `AWS::S3Tables::TableBucket` publishes `Tags`, and `CreateTableBucket` takes
            // a tag MAP of its own, so a table bucket a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::S3TablesTableBucket => {
                create_table_bucket(ctx, region, resource, propagated).await
            }
            Self::S3VectorsVectorBucket => {
                create_vector_bucket(ctx, region, resource, propagated).await
            }
            Self::S3VectorsIndex => create_index(ctx, region, resource, propagated).await,
            Self::IamRole => create_role(ctx, region, resource, propagated).await,
            // `AWS::IAM::InstanceProfile` publishes no `Tags` property and
            // `CreateInstanceProfile` takes none from this arm, so no tags are propagated to
            // a profile.
            Self::IamInstanceProfile => create_instance_profile(ctx, region, resource).await,
            Self::IamUser => create_iam_user(ctx, region, resource, propagated).await,
            // `AWS::IAM::AccessKey` publishes no `Tags` property and `CreateAccessKey` takes
            // none, so no tags are propagated to an access key.
            Self::IamAccessKey => create_iam_access_key(ctx, region, resource).await,
            // `AWS::IAM::ManagedPolicy` publishes no `Tags` property and `CreatePolicy`
            // takes none from this arm, so no tags are propagated to a policy.
            Self::IamManagedPolicy => create_managed_policy(ctx, region, resource).await,
            Self::CodePipelinePipeline => {
                create_pipeline(ctx, region, resource, propagated).await
            }
            Self::CodePipelineCustomActionType => {
                create_custom_action_type(ctx, region, resource, propagated).await
            }
            Self::EcsCluster => create_cluster(ctx, region, resource, propagated).await,
            // `AWS::ECS::Service` publishes `Tags`, which `CreateService` takes as its own
            // `tags` list, so a service a stack creates carries the tags CloudFormation gives
            // it beside the ones the declaration asks for.
            // BOXED, for the reason the other heavy arms of this match are: the call's future
            // is the owning service's whole handler, which schedules the service's tasks.
            Self::EcsService => {
                Box::pin(create_ecs_service(ctx, region, resource, propagated)).await
            }
            Self::SqsQueue => create_queue(ctx, region, resource, propagated).await,
            // Neither policy type publishes a `Tags` property — a policy is an attribute of
            // the queues it names, and those carry the tags — so no tags are propagated to
            // one.
            Self::SqsQueuePolicy => put_queue_policy(ctx, region, resource).await,
            Self::SageMakerModelPackageGroup => {
                create_model_package_group(ctx, region, resource, propagated).await
            }
            // Each of the three creates takes a `Tags` list, so a model, a notebook instance
            // and a lifecycle configuration a stack creates each carry the tags
            // CloudFormation gives them beside the ones their declaration asks for.
            Self::SageMakerModel => {
                create_sagemaker_model(ctx, region, resource, propagated).await
            }
            Self::SageMakerNotebookInstance => {
                create_sagemaker_notebook_instance(ctx, region, resource, propagated).await
            }
            Self::SageMakerNotebookInstanceLifecycleConfig => {
                create_sagemaker_notebook_instance_lifecycle_config(
                    ctx, region, resource, propagated,
                )
                .await
            }
            Self::SnsTopic => create_topic(ctx, region, resource, propagated).await,
            // Neither topic-policy type publishes a `Tags` property — a policy is an
            // attribute of the topics it names, and those carry the tags — so no tags are
            // propagated to one.
            Self::SnsTopicPolicy => put_topic_policy(ctx, region, resource).await,
            // A subscription publishes no `Tags` property — SNS holds tags on topics alone —
            // so no tags are propagated to one.
            Self::SnsSubscription => subscribe(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                create_experiment_template(ctx, region, resource, propagated).await
            }
            // `AWS::WAFRegional::IPSet` publishes `Name` and `IPSetDescriptors` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::WafRegionalIpSet => create_ip_set(ctx, region, resource).await,
            // Each WAFv2 type publishes a `Tags` property and its create takes that list as
            // its own member, so a stack's resources in WAF carry the tags CloudFormation
            // gives them. Every arm is the same call sequence under the type's own
            // operation names ([`Wafv2Calls`]).
            Self::Wafv2WebAcl => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_WEB_ACL_CALLS).await
            }
            Self::Wafv2RuleGroup => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_RULE_GROUP_CALLS)
                    .await
            }
            Self::Wafv2IpSet => {
                create_wafv2_entity(ctx, region, resource, propagated, WAFV2_IP_SET_CALLS).await
            }
            Self::Wafv2RegexPatternSet => {
                create_wafv2_entity(
                    ctx,
                    region,
                    resource,
                    propagated,
                    WAFV2_REGEX_PATTERN_SET_CALLS,
                )
                .await
            }
            // An association publishes no `Tags` property and `AssociateWebACL` takes none:
            // the binding is not a taggable resource — AWS WAF holds tags on its own
            // entities — so no tags are propagated to one.
            Self::Wafv2WebAclAssociation => associate_web_acl(ctx, region, resource).await,
            // An `AWS::Glue::Job`'s properties ARE `CreateJob`'s own members, so the
            // declared document is the request, with the one member CloudFormation decides
            // (the job's name) set on the way out and the tag MAP that call takes merged
            // from the declaration's own and the stack's.
            Self::GlueJob => create_glue_job(ctx, region, resource, propagated).await,
            // The two catalog types publish no `Tags` property at all — neither is a
            // taggable resource, AWS Glue holding tags on the catalog entities its own
            // tagging calls name — so no tags are propagated to one, and the declared
            // document IS the create's request with the name CloudFormation decided set
            // inside the input structure that call takes it in.
            Self::GlueDatabase => create_glue_database(ctx, region, resource).await,
            Self::GlueTable => create_glue_table(ctx, region, resource).await,
            // A crawler IS taggable: its `Tags` property is the tag MAP `CreateCrawler`'s
            // own `Tags` member takes, so it carries the stack's tags beside its own.
            Self::GlueCrawler => create_glue_crawler(ctx, region, resource, propagated).await,
            // `AWS::Grafana::Workspace` publishes no `Tags` property either, so the type
            // is not taggable and no stack tags are propagated to it.
            Self::GrafanaWorkspace => create_workspace(ctx, region, resource).await,
            // `AWS::OpenSearchServerless::SecurityPolicy` publishes no `Tags` property and
            // `CreateSecurityPolicy` takes none, so a policy carries no propagated tag.
            Self::OpenSearchServerlessSecurityPolicy => {
                create_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServerlessCollection => {
                create_collection(ctx, region, resource, propagated).await
            }
            Self::OpenSearchServiceDomain => {
                create_domain(ctx, region, resource, propagated).await
            }
            Self::GuardDutyDetector => {
                create_detector(ctx, region, resource, propagated).await
            }
            Self::GuardDutyIpSet => {
                create_guardduty_ip_set(ctx, region, resource, propagated).await
            }
            Self::KmsKey => create_kms_key(ctx, region, resource, propagated).await,
            // `AWS::KMS::Alias` publishes `AliasName` and `TargetKeyId` and no `Tags`: an
            // alias is not a taggable resource — KMS holds tags on KEYS — so no tags are
            // propagated to one.
            Self::KmsAlias => create_kms_alias(ctx, region, resource).await,
            Self::MskCluster => create_msk_cluster(ctx, region, resource, propagated).await,
            Self::MskServerlessCluster => {
                create_msk_serverless_cluster(ctx, region, resource, propagated).await
            }
            Self::MqBroker => create_mq_broker(ctx, region, resource, propagated).await,
            Self::RdsDbInstance => {
                create_db_instance(ctx, region, resource, propagated).await
            }
            // `AWS::ElastiCache::CacheCluster` publishes `Tags` as the `{Key, Value}` list
            // `CreateCacheCluster`'s own `Tags` member takes, so a cluster a stack creates
            // carries the tags CloudFormation gives it beside the declared ones.
            Self::ElastiCacheCacheCluster => {
                // Boxed, like every other arm of this catalogue should be as it grows: the
                // enclosing future holds one pointer instead of this arm's whole state, so
                // adding a type does not enlarge the future a nested stack's provisioning
                // walk keeps on the stack.
                Box::pin(create_cache_cluster(ctx, region, resource, propagated)).await
            }
            Self::RedshiftCluster => {
                create_redshift_cluster(ctx, region, resource, propagated).await
            }
            Self::RedshiftClusterSubnetGroup => {
                create_cluster_subnet_group(ctx, region, resource, propagated).await
            }
            // `AWS::Redshift::ClusterParameterGroup` publishes `Tags` as the `{Key, Value}`
            // list `CreateClusterParameterGroup`'s own `Tags` member takes, so a group a
            // stack creates carries the tags CloudFormation gives it beside the declared ones.
            Self::RedshiftClusterParameterGroup => {
                Box::pin(create_cluster_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                ))
                .await
            }
            Self::RdsDbSubnetGroup => {
                create_db_subnet_group(ctx, region, resource, propagated).await
            }
            Self::RdsDbParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                create_parameter_group(
                    ctx,
                    region,
                    resource,
                    propagated,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbProxy => create_db_proxy(ctx, region, resource, propagated).await,
            // `AWS::RDS::DBProxyTargetGroup` publishes no `Tags` property — Amazon RDS
            // holds tags on the PROXY, not on the registrations in its target group — so
            // the type is not taggable and no tags are propagated to one.
            Self::RdsDbProxyTargetGroup => {
                create_db_proxy_target_group(ctx, region, resource).await
            }
            // `AWS::Events::Rule` publishes `Description`, `EventBusName`, `EventPattern`,
            // `Name`, `RoleArn`, `ScheduleExpression`, `State` and `Targets` and no
            // `Tags`: the type is not taggable, so no tags are propagated to it.
            Self::EventsRule => put_rule(ctx, region, resource).await,
            // `AWS::Pipes::Pipe` publishes `Tags` as a MAP, which is the shape
            // `CreatePipe`'s own `Tags` member takes, so a pipe a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::PipesPipe => event_pipes::create_pipe(ctx, region, resource, propagated).await,
            // `AWS::ResourceGroups::Group` publishes `Tags` as the `[{Key, Value}]` list most
            // types do, which `CreateGroup` takes as its own `Tags` MAP, so a group a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::ResourceGroupsGroup => {
                create_resource_group(ctx, region, resource, propagated).await
            }
            Self::SsmParameter => put_parameter(ctx, region, resource, propagated).await,
            // `AWS::SSM::Association` publishes no `Tags` property — Systems Manager tags
            // documents and managed nodes rather than the associations between them — so the
            // type is not taggable and no tags are propagated to one.
            Self::SsmAssociation => create_ssm_association(ctx, region, resource).await,
            Self::StepFunctionsActivity => {
                create_activity(ctx, region, resource, propagated).await
            }
            Self::SchedulerScheduleGroup => {
                create_schedule_group(ctx, region, resource, propagated).await
            }
            // `AWS::Scheduler::Schedule` publishes no `Tags` property — EventBridge
            // Scheduler tags a schedule GROUP, not a schedule — so the type is not
            // taggable and no tags are propagated to it.
            Self::SchedulerSchedule => create_schedule(ctx, region, resource).await,
            // `AWS::Cognito::UserPoolGroup` publishes `Description`, `GroupName`,
            // `Precedence`, `RoleArn` and `UserPoolId` and no `Tags`: the type is not
            // taggable, so no tags are propagated to a group.
            Self::CognitoUserPoolGroup => create_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => create_acme_endpoint(ctx, region, resource, propagated).await,
            Self::AcmCertificate => request_certificate(ctx, region, resource, propagated).await,
            Self::AcmPcaCertificateAuthority => {
                create_certificate_authority(ctx, region, resource, propagated).await
            }
            // Neither `AWS::ACMPCA::Certificate` nor
            // `AWS::ACMPCA::CertificateAuthorityActivation` publishes a `Tags` property:
            // Amazon Web Services Private CA tags the certificate AUTHORITY, not a
            // certificate it issued or the import of one, so no tags are propagated to
            // either.
            Self::AcmPcaCertificate => issue_certificate(ctx, region, resource).await,
            Self::AcmPcaCertificateAuthorityActivation => {
                activate_certificate_authority(ctx, region, resource).await
            }
            Self::Route53ResolverEndpoint => {
                create_resolver_endpoint(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverRule => {
                create_resolver_rule(ctx, region, resource, propagated).await
            }
            // `AWS::Route53Resolver::ResolverRuleAssociation` publishes `Name`,
            // `ResolverRuleId` and `VPCId` and no `Tags`: the type is not taggable, so no
            // tags are propagated to an association.
            Self::Route53ResolverRuleAssociation => {
                associate_resolver_rule(ctx, region, resource).await
            }
            Self::Route53ResolverFirewallDomainList => {
                create_firewall_domain_list(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverFirewallRuleGroup => {
                create_firewall_rule_group(ctx, region, resource, propagated).await
            }
            Self::Route53ResolverFirewallRuleGroupAssociation => {
                associate_firewall_rule_group(ctx, region, resource, propagated).await
            }
            Self::Route53HostedZone => {
                create_hosted_zone(ctx, region, resource, propagated).await
            }
            // `AWS::Route53::RecordSet` publishes no `Tags` property — Route 53 tags a
            // hosted zone, not a record set — so the type is not taggable and no tags are
            // propagated to one.
            Self::Route53RecordSet => create_record_set(ctx, region, resource).await,
            // The same for a GROUP of record sets, which publishes no `Tags` property
            // either: the group IS the record sets it declares, applied in one change batch.
            Self::Route53RecordSetGroup => create_record_set_group(ctx, region, resource).await,
            // `AWS::Route53::HealthCheck` publishes `HealthCheckTags`, which the health
            // check carries beside the ones CloudFormation gives it — applied through
            // `ChangeTagsForResource`, since `CreateHealthCheck` takes no tags.
            Self::Route53HealthCheck => {
                create_health_check(ctx, region, resource, propagated).await
            }
            // None of the three types below publishes a `Tags` property — Route 53 tags a
            // hosted zone and a health check, not a CIDR collection, a delegation set or a
            // traffic policy — so no tags are propagated to one.
            Self::Route53CidrCollection => {
                create_cidr_collection(ctx, region, resource, propagated).await
            }
            Self::Route53DelegationSet => {
                create_delegation_set(ctx, region, resource, propagated).await
            }
            Self::Route53TrafficPolicy => create_traffic_policy(ctx, region, resource).await,
            // `AWS::ElasticBeanstalk::Application` publishes `ApplicationName`,
            // `Description` and `ResourceLifecycleConfig` and no `Tags`: the type is not
            // taggable, so no tags are propagated to it. The same goes for
            // `AWS::ElasticBeanstalk::ApplicationVersion`, whose properties are the
            // application it belongs to, its description and its source bundle.
            Self::ElasticBeanstalkApplication => create_eb_application(ctx, region, resource).await,
            Self::ElasticBeanstalkApplicationVersion => {
                create_eb_application_version(ctx, region, resource).await
            }
            Self::ElasticBeanstalkEnvironment => {
                create_eb_environment(ctx, region, resource, propagated).await
            }
            Self::TransferServer => {
                create_transfer_server(ctx, region, resource, propagated).await
            }
            Self::CodeDeployApplication => {
                create_codedeploy_application(ctx, region, resource, propagated).await
            }
            Self::CodeDeployDeploymentGroup => {
                create_codedeploy_deployment_group(ctx, region, resource, propagated).await
            }
            // `AWS::CodeDeploy::DeploymentConfig` publishes `ComputePlatform`,
            // `DeploymentConfigName`, `MinimumHealthyHosts`, `TrafficRoutingConfig` and
            // `ZonalConfig` and no `Tags`: the type is not taggable, so no tags are
            // propagated to a deployment configuration.
            Self::CodeDeployDeploymentConfig => {
                create_codedeploy_deployment_config(ctx, region, resource).await
            }
            Self::BatchComputeEnvironment => {
                create_compute_environment(ctx, region, resource, propagated).await
            }
            Self::BatchJobQueue => create_job_queue(ctx, region, resource, propagated).await,
            Self::BatchServiceEnvironment => {
                create_service_environment(ctx, region, resource, propagated).await
            }
            Self::EksAccessEntry => create_access_entry(ctx, region, resource, propagated).await,
            Self::EksCluster => create_eks_cluster(ctx, region, resource, propagated).await,
            Self::EksPodIdentityAssociation => {
                create_pod_identity_association(ctx, region, resource, propagated).await
            }
            Self::ApiGatewayDomainName => {
                create_apigateway_domain_name(ctx, region, resource, propagated).await
            }
            // `AWS::ApiGateway::BasePathMapping` publishes `BasePath`, `DomainName`,
            // `DomainNameId`, `RestApiId` and `Stage` and no `Tags`: the type is not
            // taggable, so no tags are propagated to a mapping.
            Self::ApiGatewayBasePathMapping => {
                create_apigateway_base_path_mapping(ctx, region, resource).await
            }
            Self::ApiGatewayV2Api => {
                create_apigatewayv2_api(ctx, region, resource, propagated).await
            }
            Self::ApiGatewayV2DomainName => {
                create_apigatewayv2_domain_name(ctx, region, resource, propagated).await
            }
            // `AWS::ApiGatewayV2::ApiMapping` publishes `ApiId`, `ApiMappingKey`,
            // `DomainName` and `Stage` and no `Tags`: the type is not taggable, so no tags
            // are propagated to a mapping.
            Self::ApiGatewayV2ApiMapping => {
                create_apigatewayv2_api_mapping(ctx, region, resource).await
            }
            // The five types whose resources live INSIDE an api, each created in the
            // collection its api holds ([`ApiGatewayV2Child`]). Only the stage's own
            // property set publishes `Tags`, and the table is what says so, so the tags
            // CloudFormation gives the resource reach the one create that takes them.
            Self::ApiGatewayV2Integration => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_INTEGRATION_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Route => {
                create_apigatewayv2_child(ctx, region, resource, propagated, &APIGATEWAYV2_ROUTE_CHILD)
                    .await
            }
            Self::ApiGatewayV2Stage => {
                create_apigatewayv2_child(ctx, region, resource, propagated, &APIGATEWAYV2_STAGE_CHILD)
                    .await
            }
            Self::ApiGatewayV2Deployment => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_DEPLOYMENT_CHILD,
                )
                .await
            }
            Self::ApiGatewayV2Authorizer => {
                create_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    propagated,
                    &APIGATEWAYV2_AUTHORIZER_CHILD,
                )
                .await
            }
            Self::CodeCommitRepository => {
                create_codecommit_repository(ctx, region, resource, propagated).await
            }
            Self::CodeArtifactDomain => {
                create_codeartifact_domain(ctx, region, resource, propagated).await
            }
            Self::CodeArtifactRepository => {
                create_codeartifact_repository(ctx, region, resource, propagated).await
            }
            Self::CodeBuildReportGroup => {
                create_codebuild_report_group(ctx, region, resource, propagated).await
            }
            // `AWS::CodeBuild::Project` publishes `Tags`, and `CreateProject` takes a tag
            // list of its own, so a project a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::CodeBuildProject => {
                create_codebuild_project(ctx, region, resource, propagated).await
            }
            // `AWS::SES::ConfigurationSet` publishes `Name`, `DeliveryOptions`,
            // `ReputationOptions`, `SendingOptions`, `SuppressionOptions`,
            // `TrackingOptions` and `VdmOptions` and no `Tags`: the type is not taggable,
            // so no tags are propagated to a configuration set.
            Self::SesConfigurationSet => {
                create_ses_configuration_set(ctx, region, resource).await
            }
            // `AWS::SES::ReceiptRuleSet` publishes `RuleSetName` alone and
            // `AWS::SES::ReceiptRule` publishes `After`, `Rule` and `RuleSetName`: neither
            // type publishes a `Tags` property — Amazon SES holds no tags on either — so no
            // tags are propagated to one.
            Self::SesReceiptRuleSet => {
                create_ses_receipt_rule_set(ctx, region, resource).await
            }
            Self::SesReceiptRule => create_ses_receipt_rule(ctx, region, resource).await,
            Self::BackupVault => {
                create_backup_vault(ctx, region, resource, propagated).await
            }
            Self::BackupPlan => create_backup_plan(ctx, region, resource, propagated).await,
            // `AWS::Backup::BackupSelection` publishes `BackupPlanId` and
            // `BackupSelection` and no tags: AWS Backup tags a plan and a vault, not a
            // selection, so no tags are propagated to one.
            Self::BackupSelection => create_backup_selection(ctx, region, resource).await,
            // `AWS::Bedrock::Guardrail` publishes `Tags`, and `CreateGuardrail` takes the
            // `TagList` of its own, so a guardrail a stack creates carries the tags
            // CloudFormation gives it beside the ones the declaration asks for.
            Self::BedrockGuardrail => {
                create_bedrock_guardrail(ctx, region, resource, propagated).await
            }
            // `AWS::Bedrock::KnowledgeBase` publishes `Tags` as a MAP, which is the shape
            // `CreateKnowledgeBase`'s own `tags` member takes, so a knowledge base a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BedrockKnowledgeBase => {
                create_bedrock_knowledge_base(ctx, region, resource, propagated).await
            }
            // `AWS::BedrockAgentCore::WorkloadIdentity` publishes `Tags`, and
            // `CreateWorkloadIdentity` takes a tag MAP of its own, so an identity a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BedrockAgentCoreWorkloadIdentity => {
                create_workload_identity(ctx, region, resource, propagated).await
            }
            // `AWS::Braket::SpendingLimit` publishes `Tags` as a MAP, which is the shape
            // `CreateSpendingLimit`'s own `tags` member takes, so a spending limit a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::BraketSpendingLimit => {
                create_braket_spending_limit(ctx, region, resource, propagated).await
            }
            // Each of the three `AWS::AmplifyUIBuilder::*` types publishes `Tags` as a MAP,
            // which is the shape its create's own `tags` member takes, so an entity a stack
            // creates carries the tags CloudFormation gives it beside the ones the declaration
            // asks for.
            Self::AmplifyUiBuilderComponent => {
                create_ui_entity(ctx, region, UiEntity::Component, resource, propagated).await
            }
            Self::AmplifyUiBuilderForm => {
                create_ui_entity(ctx, region, UiEntity::Form, resource, propagated).await
            }
            Self::AmplifyUiBuilderTheme => {
                create_ui_entity(ctx, region, UiEntity::Theme, resource, propagated).await
            }
            // `AWS::Amplify::App` publishes `Tags` as a LIST of pairs, which `CreateApp` takes
            // as the `TagMap` of its own `tags` member, so an app a stack creates carries the
            // tags CloudFormation gives it beside the ones the declaration asks for.
            Self::AmplifyApp => create_amplify_app(ctx, region, resource, propagated).await,
            Self::XraySamplingRule => {
                create_sampling_rule(ctx, region, resource, propagated).await
            }
            // `AWS::MWAA::Environment` publishes `Tags` as a MAP, which is the shape
            // `CreateEnvironment`'s own `Tags` member takes, so an environment a stack
            // creates carries the tags CloudFormation gives it beside the ones the
            // declaration asks for.
            Self::MwaaEnvironment => {
                create_mwaa_environment(ctx, region, resource, propagated).await
            }
            // None of the three `AWS::Config::*` types publishes a `Tags` property — AWS
            // Config holds tags on a recorder and on a rule through its own tagging call
            // and takes none on either put — so no tags are propagated to one.
            Self::ConfigConfigurationRecorder => {
                put_configuration_recorder(ctx, region, resource).await
            }
            Self::ConfigDeliveryChannel => put_delivery_channel(ctx, region, resource).await,
            Self::ConfigConfigRule => put_config_rule(ctx, region, resource).await,
        }
    }

    /// Those of the tags CloudFormation gives a resource that the owning service's own API
    /// will take.
    ///
    /// Every create here is made through the owning service's PUBLIC API — the same path a
    /// client's own call takes, which is what makes a stack's resource indistinguishable
    /// from one a client created. Some of those APIs refuse a tag key in AWS's reserved
    /// `aws:` namespace, which is AWS's rule for a CALLER's request ("don't use the `aws:`
    /// prefix ... it is reserved for AWS use"): IAM's `CreateRole`, CodePipeline's
    /// `CreatePipeline` and EventBridge Scheduler's `CreateScheduleGroup` all do, in their
    /// own crates' tag validation. AWS sets the
    /// `aws:cloudformation:*` tags from inside itself, through no public API: here that
    /// inside path is the in-process call this provisioning request IS, which the framework
    /// stamps and a client's request cannot be — so a type whose owning crate reads that
    /// stamp takes the three (see the paragraph below), and for a type whose crate does not
    /// they are left off rather than sent to be refused: a stack must not fail over tags
    /// CloudFormation adds of its own accord. The stack's own tags, which are ordinary keys,
    /// are applied either
    /// way. ACM is the same case: its `CreateAcmeEndpoint` refuses a reserved key on either
    /// half of a pair, and so do Amazon ECS's `CreateCluster`, Amazon MSK's two cluster
    /// creates, Amazon SQS's `CreateQueue` — which refuses it in its own crate's
    /// `state::tag_refusal`, the one check both `CreateQueue`'s `tags` map and `TagQueue`
    /// run — and Amazon Data Firehose's `CreateDeliveryStream`, whose `TagKey` shape is
    /// modelled with the reserved prefix excluded, the same check its `TagDeliveryStream`
    /// makes. CodeCommit's `CreateRepository` is in the same position: a tag key in that
    /// namespace is its `InvalidSystemTagUsageException`, and Amazon ECR Public's
    /// `CreateRepository` is the same case: a key in that namespace is the
    /// `InvalidTagParameterException` its own crate's tag validation answers, the one check
    /// its `TagResource` runs too. AWS WAF's four WAFv2 entity
    /// creates are the same case again: each takes a `TagList` and refuses a key in the
    /// reserved namespace in its own crate's tag validation, the one check `TagResource`
    /// runs too.
    ///
    /// A type whose owning service ADMITS the three on that in-process path — Amazon S3,
    /// whose crate refuses a reserved key to an API caller and takes one from a provisioning
    /// call, Amazon S3
    /// Vectors, whose crate does the same, and Amazon EventBridge Pipes, whose crate reads
    /// the same signal —
    /// is NOT in the set below: the three travel with the create, and that service's own
    /// read of the resource's tags on the handle the stack publishes answers with them. A
    /// type stays in the set only until its own crate reads that path too.
    fn applicable_tags(self, propagated: &[(String, String)]) -> Vec<(String, String)> {
        let refuses_reserved_keys = matches!(
            self,
            Self::IamRole
                | Self::IamUser
                | Self::CodePipelinePipeline
                // CodePipeline refuses a tag key in the reserved namespace on
                // `CreateCustomActionType` too, in the same tag validation its
                // `CreatePipeline` runs.
                | Self::CodePipelineCustomActionType
                | Self::EcsCluster
                // Amazon ECS refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateService`'s own `tags` list as much as `CreateCluster`'s
                // and `TagResource`, in the one check that crate's tag validation makes ("Tag
                // key must not start with aws:") — so a service carries the stack's own tags
                // without the three.
                | Self::EcsService
                | Self::EcrPublicRepository
                // Amazon ECR refuses a tag key in the reserved namespace on every call
                // that takes tags — `CreateRepository`'s `tags` and `TagResource` alike —
                // so a repository carries the stack's own tags without the three.
                | Self::EcrRepository
                | Self::FirehoseDeliveryStream
                // Amazon Kinesis Data Streams refuses a tag key in the reserved namespace
                // on every call that takes tags — `CreateStream`'s `Tags`, `TagResource` and
                // `AddTagsToStream` alike — so a stream carries the stack's own tags
                // without the three.
                | Self::KinesisStream
                | Self::SchedulerScheduleGroup
                | Self::AcmAcmeEndpoint
                // ACM refuses a tag key in the reserved namespace on every call that takes
                // tags, in the one check its own crate's tag validation makes, so a
                // certificate's are left off the same way an ACME endpoint's are.
                | Self::AcmCertificate
                // Amazon Web Services Private CA refuses a tag key in the reserved
                // namespace on `CreateCertificateAuthority` and on
                // `TagCertificateAuthority` alike, in the one check its own crate's tag
                // validation makes.
                | Self::AcmPcaCertificateAuthority
                | Self::MskCluster
                | Self::MskServerlessCluster
                | Self::SqsQueue
                | Self::CodeCommitRepository
                // KMS refuses a tag key in the reserved namespace on `CreateKey` and on
                // `TagResource` alike, in its own crate's tag validation.
                | Self::KmsKey
                | Self::Wafv2WebAcl
                | Self::Wafv2RuleGroup
                | Self::Wafv2IpSet
                | Self::Wafv2RegexPatternSet
                // Amazon API Gateway refuses a tag key in the reserved namespace on every
                // call that takes tags — the `tags` member of each create and `TagResource`
                // alike, in the one check its own crate's tag validation makes ("The tag
                // key … must not start with aws:") — so a custom domain name carries the
                // stack's own tags without the three.
                | Self::ApiGatewayDomainName
                // AWS X-Ray refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateSamplingRule`'s `Tags` and `TagResource` alike, in
                // the one check its own crate's tag validation makes ("Don't use `aws:` as
                // a prefix for keys; it's reserved for Amazon Web Services use") — so a
                // sampling rule carries the stack's own tags without the three.
                | Self::XraySamplingRule
                // Amazon MWAA refuses a tag key in the reserved namespace on every call that
                // takes tags — `CreateEnvironment`'s `Tags` map and `TagResource` alike, in
                // the one check its own crate's tag validation makes — so an environment
                // carries the stack's own tags without the three.
                | Self::MwaaEnvironment
                // Amazon ElastiCache refuses a tag key in the reserved namespace on every
                // call that takes tags — `CreateCacheCluster`'s `Tags` member and
                // `AddTagsToResource` alike, in the one check its own crate's tag validation
                // makes — so a cache cluster carries the stack's own tags without the three.
                | Self::ElastiCacheCacheCluster
                // Resource Groups refuses a tag key in the reserved namespace on every call
                // that takes tags — `CreateGroup`'s `Tags` map and `Tag` alike, in the one
                // check its own crate's tag validation makes — so a group carries the stack's
                // own tags without the three.
                | Self::ResourceGroupsGroup
        );
        propagated
            .iter()
            .filter(|(key, _)| !(refuses_reserved_keys && is_reserved_tag_key(key)))
            .cloned()
            .collect()
    }

    /// How THIS type's resource provider words a refusal of the owning service, which is the
    /// message CloudFormation quotes in the failed resource's status reason
    /// ([`super::provisioning`]).
    ///
    /// A provider reports the refusal of the service it called, and most of them report that
    /// service's message as it came — the diagnostic a client of that service would read,
    /// carrying that service's own name and status. A provider written against the
    /// CloudFormation resource-provider plugin reports it through the exception class of the
    /// `HandlerErrorCode` it classified the refusal under, and THAT class's wording is part
    /// of the message rather than only of the envelope: an `InvalidRequest` refusal reaches
    /// the reason as `Invalid request provided: <the service's message>`, which is what an
    /// `AWS::OpenSearchService::Domain` whose cluster its service will not provision is
    /// reported with.
    ///
    /// Which of the two a type is, is that type's provider's and cannot be derived from the
    /// type's name, so it is stated per type here — the same per-type half of provisioning
    /// the create and delete arms are. A type not named below reports the message unchanged.
    pub(super) fn handler_message(self, class: &str, message: &str) -> String {
        let plugin_worded = matches!(self, Self::OpenSearchServiceDomain);
        if plugin_worded && class == super::provisioning::HANDLER_ERROR_INVALID_REQUEST {
            return format!("{INVALID_REQUEST_PROVIDED}{message}");
        }
        message.to_string()
    }

    /// Which `HandlerErrorCode` class this type's provider puts a refusal carrying `code` in,
    /// where that is NOT the class the code itself falls in
    /// ([`super::provisioning::handler_error_code`] computes `classified`).
    ///
    /// `NotFound` is the class of a refusal about the resource the handler MANAGES — the one
    /// it was asked to read, update or delete. A refusal naming a DIFFERENT resource, one the
    /// declaration merely REFERENCES, says the declaration named something that is not there:
    /// the request the handler was given is the thing at fault, so its provider reports it as
    /// `InvalidRequest` and a client reading the class sees a template to fix rather than a
    /// resource that vanished.
    ///
    /// Which of a type's refusals are about a referenced resource is that type's own, so it is
    /// stated per type here beside the rest of the per-type half of provisioning. A type not
    /// named below reports the class its code falls in.
    pub(super) fn handler_error_class(self, code: &str, classified: &'static str) -> &'static str {
        let references_another_resource = match self {
            // A record set is written INSIDE a hosted zone its declaration names
            // (`HostedZoneId`/`HostedZoneName`), and the record set itself is the resource
            // this handler manages: a zone Route 53 does not hold (`NoSuchHostedZone`) is a
            // declaration naming a zone that is not there. A GROUP of record sets names its
            // zone the same way and is in the same position.
            Self::Route53RecordSet | Self::Route53RecordSetGroup => {
                code.contains(NO_SUCH_HOSTED_ZONE)
            }
            _ => false,
        };
        if references_another_resource {
            return super::provisioning::HANDLER_ERROR_INVALID_REQUEST;
        }
        classified
    }

    /// Whether the resource this provisioner CREATED has reached the state the owning
    /// service reports it ready in — read from that service, never decided here.
    ///
    /// Most creates leave the resource ready, so most types answer [`Readiness::Ready`]
    /// without asking anything: the call that created the resource already said everything
    /// there was to say about it. A type whose create is documented to leave the resource
    /// coming up is the exception, and for such a type this is what CloudFormation holds the
    /// resource's creation open on ([`super::stabilising`]) — asked once where the create
    /// answered, and again by [`super::provisioning::advance_stabilisations`] until that
    /// service reports it ready.
    ///
    /// `Err` is the owning service's own refusal of the READ, which says nothing about the
    /// resource: the caller decides what an unanswered read means.
    pub(super) async fn readiness(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
    ) -> Result<Readiness, Refusal> {
        match self {
            // "A certificate enters status PENDING_VALIDATION upon being requested"
            // (`CertificateDetail$Status`) and is usable once ACM has issued it, so the
            // resource's creation is finished when ACM reports it `ISSUED`.
            Self::AcmCertificate => certificate_readiness(ctx, region, physical_resource_id).await,
            // "Private CA is pending creation" is the status a created CA is reported in
            // (`CertificateAuthorityStatus$CREATING`), and its certificate signing request
            // is not available in it, so the resource's creation is finished once that
            // service reports the CA out of it.
            Self::AcmPcaCertificateAuthority => {
                certificate_authority_readiness(ctx, region, physical_resource_id).await
            }
            // `DomainStatus$Created` is "true if domain creation is complete" and
            // `DomainStatus$Processing` "true if OpenSearch Service is processing
            // configuration changes", so a domain the create has just answered for is still
            // coming up and the resource's creation is finished when that service reports it
            // settled.
            Self::OpenSearchServiceDomain => {
                domain_readiness(ctx, region, physical_resource_id).await
            }
            // "The volume state" a created EBS volume is reported in is `creating`
            // (`Volume$State`), and a volume cannot be attached until it is `available`, so
            // the resource's creation is finished once Amazon EC2 reports it there.
            Self::Ec2Volume => volume_readiness(ctx, region, physical_resource_id).await,
            // `CreateDeliveryStream` "is an asynchronous operation that immediately
            // returns. The initial status of the Firehose stream is CREATING. After the
            // Firehose stream is created, its status is ACTIVE and it now accepts data …
            // Attempts to send data to a delivery stream that is not in the ACTIVE state
            // cause an exception", so a stream the create has just answered for cannot take
            // the data a client writes to it and the resource's creation is finished once
            // that service reports it `ACTIVE`.
            Self::FirehoseDeliveryStream => {
                delivery_stream_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateStream` "is an asynchronous operation. Upon receiving a CreateStream
            // request, Kinesis Data Streams immediately returns and sets the stream status
            // to CREATING. After the stream is created, Kinesis Data Streams sets the stream
            // status to ACTIVE. You should perform read and write operations only on an
            // ACTIVE stream", so a stream the create has just answered for serves no read
            // or write yet and the resource's creation is finished once that service reports
            // it `ACTIVE`.
            Self::KinesisStream => {
                kinesis_stream_readiness(ctx, region, physical_resource_id).await
            }
            // "If the status is UPDATING, you cannot update the domain name"
            // (`DomainName$domainNameStatus`), and an edge-optimized domain is served by a
            // CloudFront distribution that is not in place when the create answers, so the
            // resource's creation is finished once API Gateway reports the domain
            // `AVAILABLE`.
            Self::ApiGatewayDomainName => {
                domain_name_readiness(ctx, region, physical_resource_id).await
            }
            // "A status of `available` means the proxy is ready to handle requests. Other
            // values indicate that you must wait for the proxy to be ready, or take some
            // action to resolve an issue" (`DBProxy$Status`), so a proxy the create has
            // just answered for handles no connection yet and the resource's creation is
            // finished once Amazon RDS reports it `available`.
            Self::RdsDbProxy => db_proxy_readiness(ctx, region, physical_resource_id).await,
            // "When the cluster is in the `creating` state, only cluster-level information
            // is displayed" (`DescribeCacheClusters`) and the cluster serves nothing in it,
            // so the resource's creation is finished once Amazon ElastiCache reports it
            // `available`.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_readiness(ctx, region, physical_resource_id)).await
            }
            // "When you create a new cluster, the Amazon EKS API server endpoint is private
            // to your VPC" only once the control plane is up: a created cluster is reported
            // `CREATING` and "you must wait for the cluster status to become ACTIVE before
            // you can … add nodes" (`CreateCluster` / `Cluster$status`), so the resource's
            // creation is finished once that service reports it `ACTIVE`.
            Self::EksCluster => eks_cluster_readiness(ctx, region, physical_resource_id).await,
            // `CreateKnowledgeBase` answers 202 with the knowledge base `CREATING` — "the
            // knowledge base is being created", in which it cannot be queried — so the
            // resource's creation is finished once Amazon Bedrock Agent reports it `ACTIVE`.
            Self::BedrockKnowledgeBase => {
                knowledge_base_readiness(ctx, region, physical_resource_id).await
            }
            // `CreatePipe` answers the pipe `CREATING` and `UpdatePipe` answers it
            // `UPDATING` — "the state the pipe is in" — and in neither does the pipe carry
            // the configuration the declaration asked for, so the resource is held open
            // until EventBridge Pipes reports it resting ([`event_pipes::readiness`]).
            Self::PipesPipe => event_pipes::readiness(ctx, region, physical_resource_id).await,
            // A created environment is `CREATING` — "the request to create the environment is
            // in progress" — and one whose VPC endpoints the caller owns is `PENDING` until
            // they exist, while an accepted update walks `CREATING_SNAPSHOT` and `UPDATING`;
            // in none of those does the environment carry what the deployment asked for, so
            // the resource is held open until Amazon MWAA reports it `AVAILABLE`.
            Self::MwaaEnvironment => {
                mwaa_environment_readiness(ctx, region, physical_resource_id).await
            }
            // `CreateCluster` answers a cluster `creating`, in which it serves no connection
            // and publishes no endpoint at all, so the resource's creation is finished once
            // Amazon Redshift reports it `available`.
            Self::RedshiftCluster => {
                Box::pin(redshift_cluster_readiness(ctx, region, physical_resource_id)).await
            }
            // `CreateNotebookInstance` answers the instance `Pending` — SageMaker creates
            // its network interface and launches the ML compute instance after the create
            // has answered, and the instance serves no Jupyter session and mints no
            // presigned URL until it is `InService` — so the resource is held open until
            // that service reports it there
            // ([`sagemaker_resources::notebook_instance_readiness`]).
            Self::SageMakerNotebookInstance => {
                notebook_instance_readiness(ctx, region, physical_resource_id).await
            }
            _ => Ok(Readiness::Ready),
        }
    }

    /// The attributes the owning service publishes for the resource this provisioner created
    /// now that it reports the resource UP — the ones its create answer could not carry,
    /// because the resource did not have them yet.
    ///
    /// Only a type whose create leaves the resource coming up has any ([`Self::readiness`]),
    /// and only where that type publishes an attribute the owning service fills in when the
    /// resource is running; every other type answers nothing, so its records stay exactly
    /// what its create reported.
    ///
    /// `Err` is the owning service's own refusal of the READ: the caller decides what an
    /// unanswered read means, exactly as it does for [`Self::readiness`].
    pub(super) async fn ready_attributes(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource_type: &str,
        physical_resource_id: &str,
    ) -> Result<BTreeMap<String, String>, Refusal> {
        match self {
            // `DomainStatus$Endpoint` is "the domain-specific endpoint used to submit index,
            // search, and data upload requests", and a domain still being provisioned has no
            // such endpoint — nor the dual-stack or VPC hosts beside it — so the create's
            // answer carries none of them and the type's endpoint attributes are read off the
            // domain once that service reports it settled.
            Self::OpenSearchServiceDomain => {
                domain_ready_attributes(ctx, region, resource_type, physical_resource_id).await
            }
            // "Nothing of the control plane itself exists yet" while Amazon EKS reports a
            // cluster `CREATING`: its API server endpoint, the certificate authority a
            // client verifies that endpoint with, the OIDC issuer of its identity provider
            // and the security group it makes for the control plane are all values that
            // service fills in as it provisions the control plane, so they are read off the
            // cluster once it reports it ready.
            Self::EksCluster => {
                Ok(read_eks_cluster(ctx, region, physical_resource_id)
                    .await?
                    .map(|cluster| eks_cluster_attributes(&cluster))
                    .unwrap_or_default())
            }
            // A knowledge base's `Status` and `UpdatedAt` are not the ones its create
            // answered with once that service has brought it up, so the type's attributes
            // are read off the knowledge base again when it reports it `ACTIVE`.
            Self::BedrockKnowledgeBase => {
                knowledge_base_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A pipe's `CurrentState`, its `StateReason` and the instant it was last
            // modified are not the ones its create answered with once that service has
            // brought it up, so the type's attributes are read off the pipe again when it
            // reports it resting.
            Self::PipesPipe => {
                event_pipes::ready_attributes(ctx, region, physical_resource_id).await
            }
            // An environment's web server host, the queue of its Celery Executor, its two VPC
            // endpoint services and the log group of each enabled Apache Airflow log type are
            // values Amazon MWAA fills in as it brings the environment up, so the type's
            // attributes are read off the environment again once it reports it `AVAILABLE`.
            Self::MwaaEnvironment => {
                mwaa_environment_ready_attributes(ctx, region, physical_resource_id).await
            }
            // A cache cluster has no endpoint while it is `creating`: the address a client
            // connects to is one Amazon ElastiCache fills in as it provisions the cluster,
            // so the type's endpoint attributes are read off the cluster once it reports it
            // `available`.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_ready_attributes(ctx, region, physical_resource_id)).await
            }
            // A Redshift cluster has no connection endpoint while it is `creating`: the
            // address and the port a client connects to are what Amazon Redshift publishes
            // once the cluster is serving, so the type's endpoint attributes are read off the
            // cluster when it reports it `available`.
            Self::RedshiftCluster => {
                Box::pin(redshift_cluster_ready_attributes(
                    ctx,
                    region,
                    physical_resource_id,
                ))
                .await
            }
            _ => Ok(BTreeMap::new()),
        }
    }

    /// Whether the service that owns the resource this provisioner DELETED has finished
    /// removing it — read from that service, never decided here.
    ///
    /// The mirror of [`Self::readiness`], and stated per type for the same reason: most
    /// deletes take the resource away, so most types answer [`Removal::Gone`] without asking
    /// anything, while a type whose delete its own service documents as asynchronous leaves
    /// the resource in a state it is still being removed from. For such a type this is what
    /// CloudFormation holds the stack operation open on ([`super::stabilising`]) — asked once
    /// where the delete answered, and again by
    /// [`super::provisioning::advance_stabilisations`] until that service no longer holds the
    /// resource — so no operation is reported complete while a resource it deleted is still
    /// there.
    ///
    /// `Err` is the owning service's own refusal of the READ, which says nothing about the
    /// resource: the caller decides what an unanswered read means.
    pub(super) async fn removal(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        physical_resource_id: &str,
    ) -> Result<Removal, Refusal> {
        match self {
            // "DeleteDeliveryStream is an asynchronous API. When an API request to
            // DeleteDeliveryStream succeeds, the Firehose stream is marked for deletion, and
            // it goes into the DELETING state … Removal of a Firehose stream that is in the
            // DELETING state is a low priority operation for the service", so the stream is
            // still there when the delete answers and the deletion is finished once Amazon
            // Data Firehose no longer holds it.
            Self::FirehoseDeliveryStream => {
                delivery_stream_removal(ctx, region, physical_resource_id).await
            }
            // "DeleteStream is an asynchronous operation … After a DeleteStream request, the
            // specified stream is in the DELETING state until Kinesis Data Streams completes
            // the deletion", so the stream is still there when the delete answers and the
            // deletion is finished once that service no longer holds it.
            Self::KinesisStream => {
                kinesis_stream_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteKnowledgeBase` answers 202 with the knowledge base `DELETING` — "the
            // knowledge base is being deleted" — so it is still there when the delete
            // answers and the deletion is finished once that service no longer holds it.
            Self::BedrockKnowledgeBase => {
                knowledge_base_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteEnvironment` leaves the environment `DELETING` — "the request to delete
            // the environment is in progress" — so it is still there when the delete answers
            // and the deletion is finished once Amazon MWAA no longer holds it.
            Self::MwaaEnvironment => {
                mwaa_environment_removal(ctx, region, physical_resource_id).await
            }
            // "The `DeleteCacheCluster` operation deletes a previously provisioned cluster"
            // and leaves it `deleting` while it does, so the cluster is still there when the
            // delete answers and the deletion is finished once Amazon ElastiCache no longer
            // holds it — which is what keeps a stack from deleting the subnet group the
            // cluster still sits in.
            Self::ElastiCacheCacheCluster => {
                Box::pin(cache_cluster_removal(ctx, region, physical_resource_id)).await
            }
            // "Before you can delete a notebook instance, you must call the
            // StopNotebookInstance API", so the delete stopped the instance and the
            // deletion is finished once SageMaker no longer holds it
            // ([`sagemaker_resources::notebook_instance_removal`]).
            Self::SageMakerNotebookInstance => {
                notebook_instance_removal(ctx, region, physical_resource_id).await
            }
            // `DeleteApplication` "deletes the specified application along with all
            // associated versions and configurations" and answers before it has — the
            // application is still described while the deletion runs — so it is still there
            // when the delete answers and the deletion is finished once Elastic Beanstalk no
            // longer holds it. (Its ENVIRONMENT needs no arm here: that delete waits for the
            // termination itself, because the application this stack deletes next cannot go
            // while the environment still runs; and an application VERSION is removed by the
            // call that deletes it.)
            Self::ElasticBeanstalkApplication => {
                eb_application_removal(ctx, region, physical_resource_id).await
            }
            _ => Ok(Removal::Gone),
        }
    }

    /// Apply a CHANGED configuration to a resource that already exists, through the owning
    /// service's own update API, and report what that service decided about it (the same
    /// [`Created`] an initial create answers with: the attributes it reported now).
    ///
    /// `resource` is the record of the resource as it stands — the identity its service
    /// knows it by, and the properties it was last deployed with — and `declared` is the
    /// property document this deployment declares for it. The difference between the two is
    /// what the call carries, so a property neither side changed is not re-sent.
    ///
    /// `Ok(None)` is a type this emulator has no update path for YET: nothing is asked of
    /// the owning service, and [`super::provisioning::reconcile`] reports the change as not
    /// applied rather than as a success that dropped it. `Err` is that service's own
    /// refusal of the change — including a change its API cannot make to an existing
    /// resource, which is the refusal a client would get.
    ///
    /// This is the resource's CONFIGURATION alone. The tags CloudFormation gives it travel
    /// separately, in [`Self::retag`], because they change independently of any declaration.
    pub(super) async fn update(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
    ) -> Result<Option<Created>, Refusal> {
        match self {
            Self::SqsQueue => update_queue(ctx, region, resource, declared).await.map(Some),
            // The same attribute write the create made, against the queues the NEW
            // declaration names: a queue it no longer names stops carrying the document,
            // which is how an update revokes a grant.
            Self::SqsQueuePolicy => update_queue_policy(ctx, region, resource, declared)
                .await
                .map(Some),
            // CloudWatch's own `PutMetricStream` is documented "Creates or updates a metric
            // stream", so the call that created the stream is the call that applies a
            // changed declaration to it — under the name the stack gave it, with the
            // configuration the template now declares.
            Self::CloudWatchMetricStream => {
                put_metric_stream_configuration(
                    ctx,
                    region,
                    &resource.physical_resource_id,
                    declared,
                )
                .await
                .map(Some)
            }
            // `PutDashboard` is documented "Creates a dashboard if it does not already
            // exist, or updates an existing dashboard. If you update a dashboard, the
            // entire contents are replaced with what you specify here", so the call that
            // created the dashboard is the call that applies a changed declaration to it.
            Self::CloudWatchDashboard => {
                put_dashboard_body(ctx, region, &resource.physical_resource_id, declared)
                    .await
                    .map(Some)
            }
            // `PutRule` is documented "Creates or updates the specified rule", so the call
            // that created the rule is the call that applies a changed declaration to it,
            // with the declared target list reconciled against the one it carries.
            Self::EventsRule => update_rule(ctx, region, resource, declared).await,
            // `PutParameter` is documented "Create or update a parameter in Parameter
            // Store", and `Overwrite` is what makes the second of the two: the changed
            // declaration reaches Systems Manager as a new version of the same parameter.
            Self::SsmParameter => update_parameter(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateAssociation` takes the id Systems Manager minted together with the whole
            // declaration — the document the association applies included — so a changed
            // declaration reaches that service as a new version of the same association
            // rather than replacing it.
            Self::SsmAssociation => update_ssm_association(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateGroup` "updates any of the properties for precedence, IAM role, or
            // description" of a group the pool already holds, so a changed declaration of
            // those reaches Amazon Cognito through it — while the two members that make the
            // group's identity are not members of that call at all.
            Self::CognitoUserPoolGroup => update_group(ctx, region, resource, declared).await,
            // A role's identity is its NAME, and every other property of
            // `AWS::IAM::Role` has an IAM call that changes it on the role that name
            // belongs to — the trust policy through `UpdateAssumeRolePolicy` — so a
            // changed declaration is applied to the role the stack created rather than
            // replacing it ([`update_role`]).
            Self::IamRole => update_role(ctx, region, resource, declared).await,
            // A user's identity is its NAME, and every other property of `AWS::IAM::User`
            // has an IAM call that changes it on the user that name belongs to — its path
            // through `UpdateUser`, which keeps the user's unique identifier — so a
            // changed declaration is applied to the user the stack created rather than
            // replacing it ([`update_iam_user`]).
            Self::IamUser => update_iam_user(ctx, region, resource, declared).await,
            // An access key's `Status` is the one property of a key that exists IAM can
            // change — `UpdateAccessKey` "changes the status of the specified access key
            // from Active to Inactive, or vice versa" — so a changed declaration of it is
            // applied to the key the stack created ([`update_iam_access_key`]), while the
            // user it belongs to and the `Serial` it is rotated with replace the resource
            // rather than reaching this call at all.
            Self::IamAccessKey => update_iam_access_key(ctx, region, resource, declared).await,
            // A managed policy's identity is its ARN, and the document it grants from is
            // changed by adding a new DEFAULT version to the policy that ARN names, so a
            // changed declaration is applied to the policy the stack created rather than
            // replacing it ([`update_managed_policy`]).
            Self::IamManagedPolicy => {
                update_managed_policy(ctx, region, resource, declared).await
            }
            // Every property of a KMS key that can change on an existing one has a KMS
            // call that changes it — its description, its key policy, and the two switches
            // — so a changed declaration is applied to the key the stack created rather
            // than replacing it ([`update_kms_key`]).
            Self::KmsKey => update_kms_key(ctx, region, resource, declared).await.map(Some),
            // `UpdateAlias` "associates an existing KMS alias with a different KMS key", so
            // a declaration that re-points an alias is applied to the alias the stack
            // created rather than replacing it ([`update_kms_alias`]).
            Self::KmsAlias => update_kms_alias(ctx, region, resource, declared).await,
            // Every attribute of a live subscription has one SNS call that writes it, so a
            // changed declaration of one is applied to the subscription the stack created
            // rather than dropped ([`update_subscription`]).
            Self::SnsSubscription => update_subscription(ctx, region, resource, declared).await,
            // A topic's properties ARE its attributes, and SNS has one call that writes an
            // attribute of a live topic, so a changed declaration is applied to the topic
            // the stack created — under the ARN its `Ref` resolves to — rather than
            // replacing it ([`update_topic`]).
            Self::SnsTopic => update_topic(ctx, region, resource, declared).await.map(Some),
            // The same attribute write the create made, against the topics the NEW
            // declaration names: a topic it no longer names stops carrying the document,
            // which is how an update revokes a grant.
            Self::SnsTopicPolicy => update_topic_policy(ctx, region, resource, declared)
                .await
                .map(Some),
            // A parameter group's `Parameters` map is the one property of either group type
            // Amazon RDS can change on a group that exists, and the modify is the call that
            // changes it — the same call the create makes for the values a template declares
            // — so a changed declaration is applied to the group the stack created rather
            // than dropped ([`update_parameter_group`]).
            Self::RdsDbParameterGroup => {
                update_parameter_group(ctx, region, resource, declared, DB_PARAMETER_GROUP_CALLS)
                    .await
            }
            Self::RdsDbClusterParameterGroup => {
                update_parameter_group(
                    ctx,
                    region,
                    resource,
                    declared,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // A Redshift parameter group's `Parameters` are the one part of its declaration
            // Amazon Redshift can change on a group that exists, and the modify is the call
            // that changes it — the same call the create makes for the declared values — so a
            // changed declaration is applied to the group the stack created rather than
            // dropped ([`update_cluster_parameter_group`]).
            Self::RedshiftClusterParameterGroup => {
                Box::pin(update_cluster_parameter_group(ctx, region, resource, declared)).await
            }
            // Every member of a health check's configuration that can change on a live one
            // is a member of `UpdateHealthCheck`, so a changed declaration is applied to the
            // health check the stack created — under the id its `Ref` resolves to — rather
            // than replacing it ([`update_health_check`]).
            Self::Route53HealthCheck => update_health_check(ctx, region, resource, declared).await,
            // `UpdateJob` takes the `JobUpdate` that "completely overwrites" the previous
            // job definition, and every property of `AWS::Glue::Job` but its name is a
            // member of that shape — so a changed declaration is applied to the job the
            // stack created rather than replacing it ([`update_glue_job`]).
            Self::GlueJob => update_glue_job(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateDatabase` "updates an existing database definition" and takes the same
            // `DatabaseInput` the create did, so a changed declaration is applied to the
            // database the stack created rather than replacing it; `UpdateTable` is the same
            // call for a table's `TableInput`, and every property of `AWS::Glue::Crawler`
            // but its tags is a member of `UpdateCrawler`.
            Self::GlueDatabase => update_glue_database(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::GlueTable => update_glue_table(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::GlueCrawler => update_glue_crawler(ctx, region, resource, declared)
                .await
                .map(Some),
            // EventBridge Scheduler's `UpdateSchedule` "uses all values, including empty
            // values, specified in the request and overrides the existing schedule", which
            // is exactly what a declaration states about a schedule — so the document that
            // created it is the document that applies a changed declaration to it
            // ([`update_schedule`]).
            Self::SchedulerSchedule => update_schedule(ctx, region, resource, declared).await,
            // `UpdatePipe` takes a pipe's name together with the configuration it is to
            // carry, so a changed declaration is applied to the pipe the stack created
            // rather than replacing it — less the `Source` that call takes no member for,
            // which a pipe cannot be repointed at ([`event_pipes::update_pipe`]).
            Self::PipesPipe => event_pipes::update_pipe(ctx, region, resource, declared).await,
            // A schedule group publishes its `Name`, which IS the group's identity, and its
            // `Tags`, which are no part of any call that changes a group that exists and
            // reach EventBridge Scheduler through its own tagging calls ([`Self::retag`]) —
            // so a declaration whose change is its tags has nothing else to ask of that
            // service ([`update_schedule_group`]).
            Self::SchedulerScheduleGroup => Ok(update_schedule_group(resource, declared)),
            // `UpdateGroup` changes a resource group's description and `UpdateGroupQuery` its
            // resource query, which are the two halves of a group a declaration can state
            // differently; its name is its identity and its tags reach that service through
            // its own tagging calls ([`Self::retag`]).
            Self::ResourceGroupsGroup => {
                update_resource_group(ctx, region, resource, declared).await
            }
            // `UpdateWorkloadIdentity` takes an identity's name together with the whole
            // list of allowed OAuth2 return URLs, which "replaces the existing list" — so
            // a changed declaration of that list is applied to the identity the stack
            // created rather than replacing it, and a declaration that states none leaves
            // it with none ([`update_workload_identity`]).
            // `UpdateGuardrail` "updates a guardrail with the values you specify", so a
            // changed declaration is applied to the guardrail the stack created rather
            // than replacing it; the guardrail's identity is the ARN Amazon Bedrock
            // minted, which an update leaves alone.
            Self::BedrockGuardrail => update_bedrock_guardrail(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateKnowledgeBase` takes a knowledge base's id together with the whole
            // configuration it is to carry, so a changed declaration is applied to the
            // knowledge base the stack created rather than replacing it; its identity is the
            // id Amazon Bedrock Agent minted, which an update leaves alone. A changed tag
            // map is no part of that call, which that arm reports as not applied.
            Self::BedrockKnowledgeBase => {
                update_bedrock_knowledge_base(ctx, region, resource, declared).await
            }
            Self::BedrockAgentCoreWorkloadIdentity => {
                update_workload_identity(ctx, region, resource, declared)
                    .await
                    .map(Some)
            }
            // Each of the three AWS Config puts is documented to create OR update the
            // resource it names, so the call that created the recorder, the channel or the
            // rule is the call that applies a changed declaration to it — under the name
            // the stack gave it, with the configuration the template now declares.
            Self::ConfigConfigurationRecorder => put_recorder_configuration(
                ctx,
                region,
                &resource.physical_resource_id,
                declared,
            )
            .await
            .map(Some),
            Self::ConfigDeliveryChannel => put_channel_configuration(
                ctx,
                region,
                &resource.physical_resource_id,
                declared,
            )
            .await
            .map(Some),
            Self::ConfigConfigRule => {
                put_config_rule_configuration(ctx, region, &resource.physical_resource_id, declared)
                    .await
                    .map(Some)
            }
            // `UpdateDomainConfig` "modifies the cluster configuration of the specified
            // Amazon OpenSearch Service domain", and every property of
            // `AWS::OpenSearchService::Domain` that is a member of it can change on a domain
            // that exists — so a changed declaration is applied to the domain the stack
            // created, which keeps its name, its id, its ARN and the address it is reached
            // at, rather than replacing it ([`update_domain`]).
            Self::OpenSearchServiceDomain => update_domain(ctx, region, resource, declared).await,
            // `PutBucketPolicy` "applies an Amazon S3 bucket policy to an Amazon S3
            // bucket", replacing the one the bucket carries, so the call that attached the
            // policy is the call that applies a changed declaration to it.
            Self::S3BucketPolicy => set_bucket_policy(ctx, region, declared).await.map(Some),
            // Amazon EC2 Auto Scaling's own `UpdateAutoScalingGroup` is the call that
            // "updates the configuration for the specified Auto Scaling group", so a changed
            // declaration reaches the group through it — under the name the group was created
            // with, which is this resource's physical id.
            Self::AutoScalingGroup => {
                update_auto_scaling_group(ctx, region, resource, declared).await
            }
            // A LAUNCH TEMPLATE is a versioned document: `CreateLaunchTemplateVersion`
            // "creates a new version of a launch template", so a changed declaration of its
            // launch data reaches Amazon EC2 as a new version of the template the stack
            // created — under the `lt-` id that template keeps
            // ([`update_ec2_launch_template`]).
            Self::Ec2LaunchTemplate => {
                update_ec2_launch_template(ctx, region, resource, declared).await
            }
            // `UpdateSpendingLimit` "updates an existing spending limit" by the ARN Amazon
            // Braket minted, taking the limit's amount and its period — every property of
            // the type except the device it caps, which replaces the resource instead — so a
            // changed declaration of either reaches the limit the stack created.
            // Amazon ECS's own `UpdateService` "modifies the parameters of a service", so a
            // changed declaration of a member it takes is applied to the service the stack
            // created rather than replacing it.
            Self::EcsService => Box::pin(update_ecs_service(ctx, region, resource, declared))
                .await
                .map(Some),
            // CloudWatch's own `PutMetricAlarm` is documented "Creates or updates an alarm",
            // so a changed declaration reaches that service as the configuration the
            // template now declares, under the name the alarm was created with.
            Self::CloudWatchAlarm => update_metric_alarm(ctx, region, resource, declared)
                .await
                .map(Some),
            Self::BraketSpendingLimit => update_braket_spending_limit(ctx, region, resource, declared)
                .await
                .map(Some),
            // `UpdateEnvironment` reconfigures the environment the stack created, by the name
            // Amazon MWAA knows it by, taking every member of the create but the ones fixed
            // when an environment is made — which replace the resource instead — so a changed
            // declaration is applied to that environment rather than replacing it.
            Self::MwaaEnvironment => update_mwaa_environment(ctx, region, resource, declared).await,
            // `UpdateApi` ("updates an Api resource") is the one call that changes an api API
            // Gateway v2 already holds, and every property of
            // `AWS::ApiGatewayV2::Api` but its protocol, its definition and its tags is a
            // member of it — so a changed declaration of one is applied to the api the stack
            // created, which keeps the `ApiId` that service minted and the endpoint it serves
            // the api on, rather than replacing it. A declaration carrying a DEFINITION is
            // applied by `ReimportApi` instead, which puts that definition over the same api
            // ([`update_apigatewayv2_api`]).
            // BOXED, for the reason the other heavy arms of this match are: the call's future
            // is the owning service's whole handler, and holding one of those inline in this
            // match's state machine is what takes a deployment past the thread's stack.
            Self::ApiGatewayV2Api => {
                Box::pin(update_apigatewayv2_api(ctx, region, resource, declared)).await
            }
            // Each of the resources that live INSIDE an api has its own patch, addressed by
            // the api that holds it and the identifier that service minted for it, and taking
            // the same members its create does — so a changed declaration of an integration,
            // a route, a stage or an authorizer is applied to the resource the stack created
            // rather than refused ([`update_apigatewayv2_child`]). Boxed for the reason the
            // api's own arm above is.
            Self::ApiGatewayV2Integration => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_INTEGRATION_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2Route => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_ROUTE_CHILD,
                ))
                .await
            }
            Self::ApiGatewayV2Stage => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_STAGE_CHILD,
                ))
                .await
            }
            // An `AWS::ApiGatewayV2::Deployment` has NO arm here: the one member
            // `UpdateDeployment` takes is the deployment's description, and this emulator does
            // not serve that operation yet (its own scaffold), so there is nothing to ask API
            // Gateway v2 for — a changed declaration is reported as not applied rather than
            // sent to a call that answers 501. A declaration that names another api or another
            // stage is create-only either way
            // ([`super::super::template::replacement_properties`]), so such a change replaces
            // the deployment.
            Self::ApiGatewayV2Authorizer => {
                Box::pin(update_apigatewayv2_child(
                    ctx,
                    region,
                    resource,
                    declared,
                    &APIGATEWAYV2_AUTHORIZER_CHILD,
                ))
                .await
            }
            // Each `Update…` takes the entity's whole declared configuration by the id Amplify
            // UI Builder minted for it, in the app environment that is fixed when the entity
            // is made — a declaration restating either replaces the resource instead — so a
            // changed declaration is applied to that entity rather than replacing it.
            Self::AmplifyUiBuilderComponent => {
                update_ui_entity(ctx, region, UiEntity::Component, resource, declared).await
            }
            Self::AmplifyUiBuilderForm => {
                update_ui_entity(ctx, region, UiEntity::Form, resource, declared).await
            }
            Self::AmplifyUiBuilderTheme => {
                update_ui_entity(ctx, region, UiEntity::Theme, resource, declared).await
            }
            // `UpdateApp` takes the app's whole declared configuration by the id AWS Amplify
            // minted for it, the app's name included, so a changed declaration is applied to
            // that app rather than replacing it.
            Self::AmplifyApp => update_amplify_app(ctx, region, resource, declared).await,
            // Every other type's in-place update is its own entry here, exactly as its
            // create and delete are: until one is written, nothing is asked of the owning
            // service and the deployment says so.
            _ => Ok(None),
        }
    }

    /// Bring the tags of a resource the stack ALREADY has back into step with the ones
    /// CloudFormation gives it ([`Self::create`]'s `propagated`), through the owning
    /// service's own tagging API.
    ///
    /// A stack operation's `Tags` are "propagated to the resources that CloudFormation
    /// creates in the stack", and a deployment that states them differently states them for
    /// the resources the stack already has as much as for the ones it creates — which is why
    /// this is asked of every carried-forward resource rather than only of one whose
    /// declaration changed: a stack's tags change without any resource's declaration
    /// changing at all.
    ///
    /// `Ok(())` for a type with no arm here: nothing is asked of its service, exactly as a
    /// type with no create arm is left declared-only. Each arm is expected to be
    /// IDEMPOTENT — it is called on every deployment — so a resource whose tags already
    /// agree with the set has nothing called about it.
    ///
    /// `declared` is the property document this deployment declares for the resource, whose
    /// own `Tags` are part of the set the resource must end up carrying.
    pub(super) async fn retag(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        declared: &Value,
        propagated: &[(String, String)],
    ) -> Result<(), Refusal> {
        let propagated = &self.applicable_tags(propagated);
        match self {
            // `PutDashboard`'s `Tags` member cannot carry a change to a dashboard's tags:
            // that member is documented as ignored when the call updates a dashboard that
            // exists, so the change reaches CloudWatch through its own tagging calls.
            Self::CloudWatchDashboard => {
                retag_dashboard(ctx, region, resource, declared, propagated).await
            }
            // A role's tags are IAM's own `TagRole`/`UntagRole`, which is also why
            // [`update_role`] does not carry them: this is the one place the tag set a role
            // is to end up with is reconciled.
            Self::IamRole => retag_role(ctx, region, resource, declared, propagated).await,
            // A user's tags are IAM's own `TagUser`/`UntagUser`, for the same reason a
            // role's are not carried by [`update_iam_user`].
            Self::IamUser => retag_iam_user(ctx, region, resource, declared, propagated).await,
            // A key's tags are KMS's own `TagResource`/`UntagResource`, which is why
            // [`update_kms_key`] does not carry them either.
            Self::KmsKey => retag_kms_key(ctx, region, resource, declared, propagated).await,
            // A topic's tags are SNS's own `TagResource`/`UntagResource` — `Tags` is a
            // member of `CreateTopic` and no attribute of a topic — which is why
            // [`update_topic`] does not carry them.
            Self::SnsTopic => retag_topic(ctx, region, resource, declared, propagated).await,
            // A schedule group's tags are EventBridge Scheduler's own `TagResource` and
            // `UntagResource`: `CreateScheduleGroup`'s `Tags` member applies to the group
            // that call creates and to no group that already exists, so this is where the
            // set a group is to end up carrying is reconciled.
            Self::SchedulerScheduleGroup => {
                retag_schedule_group(ctx, region, resource, declared, propagated).await
            }
            // A resource group's tags are Resource Groups' own `Tag` and `Untag`:
            // `CreateGroup`'s `Tags` member applies to the group that call creates and to no
            // group that already exists, so this is where the set a group is to end up
            // carrying is reconciled.
            Self::ResourceGroupsGroup => {
                retag_resource_group(ctx, region, resource, declared, propagated).await
            }
            // A stream's tags are Amazon Kinesis Data Streams' own `AddTagsToStream` and
            // `RemoveTagsFromStream`: `CreateStream`'s `Tags` member applies to the stream
            // that call creates and to no stream that already exists, so this is where the
            // set a stream is to end up carrying is reconciled.
            Self::KinesisStream => {
                retag_kinesis_stream(ctx, region, resource, declared, propagated).await
            }
            // An environment's tags are Amazon MWAA's own `TagResource` and `UntagResource`:
            // `CreateEnvironment`'s `Tags` member applies to the environment that call
            // creates, and `UpdateEnvironment` carries no tags at all, so this is where the
            // set an environment is to end up carrying is reconciled.
            Self::MwaaEnvironment => {
                retag_mwaa_environment(ctx, region, resource, declared, propagated).await
            }
            // Every other type's re-tagging is its own entry here, for the same reason its
            // create, update and delete are: the tag set is the same for every type, but the
            // calls that apply it are each service's own.
            _ => Ok(()),
        }
    }

    /// Delete one resource from the service that owns it. A resource that service no
    /// longer holds is already in the state the delete asks for, so each arm decides that
    /// on the answer's own terms: on its not-found CODE where the service models one —
    /// never on a message, which carries the request's own name — and, where it models
    /// none (SageMaker's model group), by READING the resource back.
    ///
    /// The whole record is passed, not just its physical id: a type whose identity is a
    /// PAIR (a security policy's `(type, name)`) needs the property that carries the other
    /// half.
    ///
    /// `final_snapshot` is the identifier CloudFormation asks for a SNAPSHOT of the resource
    /// under, for a resource whose `DeletionPolicy` is `Snapshot`
    /// ([`super::provisioning::final_snapshot_identifier`]). It reaches the arms of the types
    /// whose owning service takes a snapshot ON THE WAY OUT, as a member of the delete call
    /// itself (Amazon RDS's `FinalDBSnapshotIdentifier`, Amazon Redshift's
    /// `FinalClusterSnapshotIdentifier`) — the published list of types `Snapshot` supports is
    /// wider than that, and a type whose snapshot is a call of its own is not one of these
    /// arms yet, so it is deleted with no snapshot taken rather than with one invented for
    /// it.
    pub(super) async fn delete(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
        final_snapshot: Option<&str>,
    ) -> Result<(), Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                delete_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::KinesisStream => {
                delete_kinesis_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                delete_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchDashboard => {
                delete_dashboard(ctx, region, physical_resource_id).await
            }
            Self::EcrPublicRepository => {
                delete_public_repository(ctx, region, physical_resource_id).await
            }
            Self::EcrRepository => delete_repository(ctx, region, resource).await,
            Self::Ec2Vpc => delete_ec2_vpc(ctx, region, physical_resource_id).await,
            Self::Ec2Subnet => delete_ec2_subnet(ctx, region, physical_resource_id).await,
            Self::Ec2SecurityGroup => {
                delete_ec2_security_group(ctx, region, physical_resource_id).await
            }
            Self::Ec2Volume => delete_ec2_volume(ctx, region, physical_resource_id).await,
            Self::Ec2LaunchTemplate => {
                delete_ec2_launch_template(ctx, region, physical_resource_id).await
            }
            Self::AutoScalingGroup => {
                delete_auto_scaling_group(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServerlessSecurityPolicy => {
                delete_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServerlessCollection => {
                delete_collection(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServiceDomain => {
                delete_domain(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => delete_bucket(ctx, region, physical_resource_id).await,
            // The policy is the named BUCKET's, and only the resource's own declaration
            // says which bucket that is.
            Self::S3BucketPolicy => delete_bucket_policy(ctx, region, resource).await,
            Self::S3TablesTableBucket => {
                delete_table_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsVectorBucket => {
                delete_vector_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsIndex => delete_index(ctx, region, physical_resource_id).await,
            Self::EmrCluster => terminate_job_flow(ctx, region, physical_resource_id).await,
            // The role's policies have to go first, and only the resource's own record
            // says which ones there are.
            Self::IamRole => delete_role(ctx, region, resource).await,
            // The roles the profile holds have to be taken out of it first, and only the
            // resource's own record says which ones there are.
            Self::IamInstanceProfile => delete_instance_profile(ctx, region, resource).await,
            // The user's password, policies and memberships have to go first, and only the
            // resource's own record says which ones there are.
            Self::IamUser => delete_iam_user(ctx, region, resource).await,
            // The key is addressed by the PAIR IAM keys one by — the user that holds it and
            // the id IAM minted — and the user half is on the resource's own declaration.
            Self::IamAccessKey => delete_iam_access_key(ctx, region, resource).await,
            // The policy's attachments and its non-default versions have to go first, and
            // only the resource's own record says which entities it was attached to.
            Self::IamManagedPolicy => delete_managed_policy(ctx, region, resource).await,
            Self::CodePipelinePipeline => {
                delete_pipeline(ctx, region, physical_resource_id).await
            }
            // The registration is addressed by the triple its declaration states, not by
            // a name, so the delete takes the whole record rather than the id.
            Self::CodePipelineCustomActionType => {
                delete_custom_action_type(ctx, region, resource).await
            }
            Self::EcsCluster => delete_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => delete_queue(ctx, region, physical_resource_id).await,
            Self::SqsQueuePolicy => delete_queue_policy(ctx, region, resource).await,
            Self::SageMakerModelPackageGroup => {
                delete_model_package_group(ctx, region, physical_resource_id).await
            }
            Self::SageMakerModel => {
                delete_sagemaker_model(ctx, region, physical_resource_id).await
            }
            // The instance has to be stopped before SageMaker will take it away, which is
            // what that arm asks for; the removal below finishes the deletion.
            Self::SageMakerNotebookInstance => {
                delete_sagemaker_notebook_instance(ctx, region, physical_resource_id).await
            }
            Self::SageMakerNotebookInstanceLifecycleConfig => {
                delete_sagemaker_notebook_instance_lifecycle_config(
                    ctx,
                    region,
                    physical_resource_id,
                )
                .await
            }
            Self::SnsTopic => delete_topic(ctx, region, physical_resource_id).await,
            Self::SnsTopicPolicy => delete_topic_policy(ctx, region, resource).await,
            Self::SnsSubscription => unsubscribe(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                delete_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => delete_ip_set(ctx, region, physical_resource_id).await,
            // WAF addresses an entity by the triple its create fixed — the declared name
            // and scope and the id that service assigned — so the delete takes the whole
            // record rather than the id.
            Self::Wafv2WebAcl => {
                delete_wafv2_entity(ctx, region, resource, WAFV2_WEB_ACL_CALLS).await
            }
            Self::Wafv2RuleGroup => {
                delete_wafv2_entity(ctx, region, resource, WAFV2_RULE_GROUP_CALLS).await
            }
            Self::Wafv2IpSet => {
                delete_wafv2_entity(ctx, region, resource, WAFV2_IP_SET_CALLS).await
            }
            Self::Wafv2RegexPatternSet => {
                delete_wafv2_entity(ctx, region, resource, WAFV2_REGEX_PATTERN_SET_CALLS).await
            }
            // The binding is addressed by the PROTECTED resource alone — "a resource has at
            // most one web ACL in front of it" — which the resource's own declaration
            // carries.
            Self::Wafv2WebAclAssociation => disassociate_web_acl(ctx, region, resource).await,
            Self::GlueJob => delete_glue_job(ctx, region, physical_resource_id).await,
            // A database is addressed by its name in the catalog its declaration names, and
            // a table by that catalog, the database that holds it and its own name — so both
            // deletes take the whole record rather than the id.
            Self::GlueDatabase => delete_glue_database(ctx, region, resource).await,
            Self::GlueTable => delete_glue_table(ctx, region, resource).await,
            Self::GlueCrawler => delete_glue_crawler(ctx, region, physical_resource_id).await,
            Self::GrafanaWorkspace => {
                delete_workspace(ctx, region, physical_resource_id).await
            }
            Self::GuardDutyDetector => delete_detector(ctx, region, physical_resource_id).await,
            // A trusted IP list is addressed by the PAIR that identifies it — the detector
            // its declaration names and the id GuardDuty minted — so the delete takes the
            // whole record rather than the id.
            Self::GuardDutyIpSet => delete_guardduty_ip_set(ctx, region, resource).await,
            // A KMS key is not deleted at once: its deletion is SCHEDULED, for the waiting
            // period the declaration asks for.
            Self::KmsKey => {
                let properties = resource.properties.as_object().cloned().unwrap_or_default();
                schedule_kms_key_deletion(ctx, region, physical_resource_id, &properties).await
            }
            // An alias is deleted at once, and deleting it leaves the key it pointed at as
            // it is ("These operations don't affect the underlying KMS key").
            Self::KmsAlias => delete_kms_alias(ctx, region, physical_resource_id).await,
            // One `DeleteCluster` deletes a cluster of either kind, addressed by the ARN
            // Amazon MSK minted for it.
            Self::MskCluster | Self::MskServerlessCluster => {
                delete_msk_cluster(ctx, region, physical_resource_id).await
            }
            Self::MqBroker => delete_mq_broker(ctx, region, physical_resource_id).await,
            // The delete carries one member off the resource's own declaration
            // (`DeleteAutomatedBackups`), so it takes the record rather than the id — and
            // the final snapshot a `Snapshot` deletion policy asks for is a member of that
            // same call.
            Self::RdsDbInstance => {
                delete_db_instance(ctx, region, resource, final_snapshot).await
            }
            // The final snapshot a `Snapshot` deletion policy asks for is a member of the
            // delete itself, so the arm takes it beside the cluster's identifier.
            Self::ElastiCacheCacheCluster => {
                Box::pin(delete_cache_cluster(
                    ctx,
                    region,
                    physical_resource_id,
                    final_snapshot,
                ))
                .await
            }
            Self::RedshiftCluster => {
                delete_redshift_cluster(ctx, region, physical_resource_id, final_snapshot)
                    .await
            }
            Self::RedshiftClusterParameterGroup => {
                delete_cluster_parameter_group(ctx, region, physical_resource_id).await
            }
            Self::RedshiftClusterSubnetGroup => {
                delete_cluster_subnet_group(ctx, region, physical_resource_id).await
            }
            Self::RdsDbSubnetGroup => {
                delete_db_subnet_group(ctx, region, physical_resource_id).await
            }
            Self::RdsDbParameterGroup => {
                delete_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                delete_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // The proxy is addressed by the name it was created under, which is this
            // resource's physical id.
            Self::RdsDbProxy => delete_db_proxy(ctx, region, physical_resource_id).await,
            // A registration is addressed by the PROXY and the target group that hold it,
            // and only the resource's own declaration carries either, so the delete takes
            // the record rather than its id — which is the group's ARN.
            Self::RdsDbProxyTargetGroup => {
                delete_db_proxy_target_group(ctx, region, resource).await
            }
            // A rule still carrying targets cannot be deleted, and only the rule's own
            // record says which bus it is on, so the delete takes the record.
            Self::EventsRule => delete_rule(ctx, region, resource).await,
            Self::SsmParameter => delete_parameter(ctx, region, physical_resource_id).await,
            Self::SsmAssociation => {
                delete_ssm_association(ctx, region, physical_resource_id).await
            }
            Self::StepFunctionsActivity => {
                delete_activity(ctx, region, physical_resource_id).await
            }
            Self::SchedulerScheduleGroup => {
                delete_schedule_group(ctx, region, physical_resource_id).await
            }
            // A resource group is addressed by its name, which is this resource's physical id.
            Self::ResourceGroupsGroup => {
                delete_resource_group(ctx, region, physical_resource_id).await
            }
            // A schedule is addressed by its name AND the group that holds it, which only
            // the resource's own record carries, so the delete takes the record.
            Self::SchedulerSchedule => delete_schedule(ctx, region, resource).await,
            // A pipe is addressed by its name alone, which is this resource's physical id.
            Self::PipesPipe => {
                event_pipes::delete_pipe(ctx, region, physical_resource_id).await
            }
            // The delete addresses the group by the PAIR that identifies it, and the pool
            // half of that pair is on the resource's own declaration.
            Self::CognitoUserPoolGroup => delete_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => delete_acme_endpoint(ctx, region, physical_resource_id).await,
            Self::AcmCertificate => delete_certificate(ctx, region, physical_resource_id).await,
            Self::AcmPcaCertificateAuthority => {
                delete_certificate_authority(ctx, region, physical_resource_id).await
            }
            // Amazon Web Services Private CA publishes no call that un-issues a
            // certificate one of its CAs signed, so removing the resource that asked for
            // it removes the stack's record of it and nothing else.
            Self::AcmPcaCertificate => Ok(()),
            // An activation is not a resource of that service's own: what it DID was put
            // the CA's certificate into it and leave it able to issue. Removing it
            // withdraws that, which is `UpdateCertificateAuthority` putting the CA back in
            // `DISABLED` — the status a CA that holds a certificate but may not use it is
            // in, and the one the CA's own resource can then be deleted from.
            Self::AcmPcaCertificateAuthorityActivation => {
                deactivate_certificate_authority(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverEndpoint => {
                delete_resolver_endpoint(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverRule => {
                delete_resolver_rule(ctx, region, physical_resource_id).await
            }
            // The disassociate addresses the association by the PAIR that identifies it,
            // which only the resource's own record carries.
            Self::Route53ResolverRuleAssociation => {
                disassociate_resolver_rule(ctx, region, resource).await
            }
            Self::Route53ResolverFirewallDomainList => {
                delete_firewall_domain_list(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallRuleGroup => {
                delete_firewall_rule_group(ctx, region, physical_resource_id).await
            }
            // The disassociate addresses the association by the id Route 53 Resolver minted
            // for it, which is this resource's physical id.
            Self::Route53ResolverFirewallRuleGroupAssociation => {
                disassociate_firewall_rule_group(ctx, region, physical_resource_id).await
            }
            Self::Route53HostedZone => {
                delete_hosted_zone(ctx, region, physical_resource_id).await
            }
            // Route 53 removes a record set by its own values, which only the resource's
            // own record carries — as is the zone its declaration names.
            Self::Route53RecordSet => delete_record_set(ctx, region, resource).await,
            // The same batch that created the group's record sets, removing them.
            Self::Route53RecordSetGroup => delete_record_set_group(ctx, region, resource).await,
            Self::Route53HealthCheck => {
                delete_health_check(ctx, region, physical_resource_id).await
            }
            // Each of the three is addressed by the identifier Route 53 minted for it, which
            // is this resource's physical id.
            Self::Route53CidrCollection => {
                delete_cidr_collection(ctx, region, physical_resource_id).await
            }
            Self::Route53DelegationSet => {
                delete_delegation_set(ctx, region, physical_resource_id).await
            }
            Self::Route53TrafficPolicy => {
                delete_traffic_policy(ctx, region, physical_resource_id).await
            }
            Self::ElasticBeanstalkApplication => {
                delete_eb_application(ctx, region, physical_resource_id).await
            }
            // An application version is keyed by the PAIR that identifies it, and the
            // application half of that pair is on the resource's own declaration.
            Self::ElasticBeanstalkApplicationVersion => {
                delete_eb_application_version(ctx, region, resource).await
            }
            Self::ElasticBeanstalkEnvironment => {
                terminate_eb_environment(ctx, region, physical_resource_id).await
            }
            // The server is addressed by the `ServerId` Transfer Family assigned it,
            // which this type's `ServerId` attribute carries — its physical id is the
            // server's ARN.
            Self::TransferServer => delete_transfer_server(ctx, region, resource).await,
            Self::CodeDeployApplication => {
                delete_codedeploy_application(ctx, region, physical_resource_id).await
            }
            // The delete addresses the group by the PAIR that identifies it, and the
            // application half of that pair is on the resource's own declaration.
            Self::CodeDeployDeploymentGroup => {
                delete_codedeploy_deployment_group(ctx, region, resource).await
            }
            Self::CodeDeployDeploymentConfig => {
                delete_codedeploy_deployment_config(ctx, region, physical_resource_id).await
            }
            Self::BatchComputeEnvironment => {
                delete_batch_compute_environment(ctx, region, physical_resource_id).await
            }
            Self::BatchJobQueue => {
                delete_batch_job_queue(ctx, region, physical_resource_id).await
            }
            Self::BatchServiceEnvironment => {
                delete_batch_service_environment(ctx, region, physical_resource_id).await
            }
            // The delete addresses the entry by the PAIR Amazon EKS keys one by, both
            // halves of which are on the resource's own declaration.
            Self::EksAccessEntry => delete_access_entry(ctx, region, resource).await,
            Self::EksCluster => delete_eks_cluster(ctx, region, physical_resource_id).await,
            // The delete addresses the association by the cluster that holds it and the id
            // Amazon EKS assigned it, both of which are on the resource's own record.
            Self::EksPodIdentityAssociation => {
                delete_pod_identity_association(ctx, region, resource).await
            }
            Self::ApiGatewayDomainName => {
                delete_apigateway_domain_name(ctx, region, physical_resource_id).await
            }
            // The delete addresses the mapping by the PAIR API Gateway keys one by — the
            // domain name and the base path — both of which are on the resource's own
            // declaration.
            Self::ApiGatewayBasePathMapping => {
                delete_apigateway_base_path_mapping(ctx, region, resource).await
            }
            Self::ApiGatewayV2Api => {
                delete_apigatewayv2_api(ctx, region, physical_resource_id).await
            }
            Self::ApiGatewayV2DomainName => {
                delete_apigatewayv2_domain_name(ctx, region, physical_resource_id).await
            }
            // The delete addresses the mapping by the PAIR API Gateway v2 keys one by, and
            // the domain half of that pair is on the resource's own declaration.
            Self::ApiGatewayV2ApiMapping => {
                delete_apigatewayv2_api_mapping(ctx, region, resource).await
            }
            // Each addressed by the api that holds it and its own identifier, both of
            // which the resource's record carries.
            Self::ApiGatewayV2Integration => {
                delete_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_INTEGRATION_CHILD)
                    .await
            }
            Self::ApiGatewayV2Route => {
                delete_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_ROUTE_CHILD).await
            }
            Self::ApiGatewayV2Stage => {
                delete_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_STAGE_CHILD).await
            }
            Self::ApiGatewayV2Deployment => {
                delete_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_DEPLOYMENT_CHILD)
                    .await
            }
            Self::ApiGatewayV2Authorizer => {
                delete_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_AUTHORIZER_CHILD)
                    .await
            }
            Self::CodeCommitRepository => {
                delete_codecommit_repository(ctx, region, physical_resource_id).await
            }
            // Neither CodeArtifact type is addressed by its physical id: each type's `Ref`
            // is the resource's ARN, so the names its delete takes come off the resource's
            // own record ([`declared_name`], [`codeartifact_property`]).
            Self::CodeArtifactDomain => {
                delete_codeartifact_domain(ctx, region, &declared_name(resource)).await
            }
            Self::CodeArtifactRepository => {
                delete_codeartifact_repository(
                    ctx,
                    region,
                    &codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY),
                    &codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY),
                    &declared_name(resource),
                )
                .await
            }
            // The delete addresses the group by the ARN CodeBuild keyed it by, which the
            // resource's own record carries.
            Self::CodeBuildReportGroup => {
                delete_codebuild_report_group(ctx, region, resource).await
            }
            // A project is addressed by the NAME it was created under, which is this
            // resource's physical id.
            Self::CodeBuildProject => {
                delete_codebuild_project(ctx, region, physical_resource_id).await
            }
            Self::SesConfigurationSet => {
                delete_ses_configuration_set(ctx, region, physical_resource_id).await
            }
            Self::SesReceiptRuleSet => {
                delete_ses_receipt_rule_set(ctx, region, physical_resource_id).await
            }
            // A rule is addressed by its own name AND the set that holds it, and only the
            // resource's own declaration carries that set.
            Self::SesReceiptRule => delete_ses_receipt_rule(ctx, region, resource).await,
            Self::BackupVault => {
                delete_backup_vault(ctx, region, physical_resource_id).await
            }
            Self::BackupPlan => delete_backup_plan(ctx, region, physical_resource_id).await,
            // The delete addresses the selection by the PAIR AWS Backup keys one by, and
            // the plan half of that pair is on the resource's own declaration.
            Self::BackupSelection => delete_backup_selection(ctx, region, resource).await,
            // The delete addresses the guardrail by the identifier Amazon Bedrock keyed it
            // by, which is this resource's physical id.
            Self::BedrockGuardrail => {
                delete_bedrock_guardrail(ctx, region, physical_resource_id).await
            }
            // The delete addresses the knowledge base by the id Amazon Bedrock Agent minted
            // for it, which is this resource's physical id.
            Self::BedrockKnowledgeBase => {
                delete_bedrock_knowledge_base(ctx, region, physical_resource_id).await
            }
            Self::BedrockAgentCoreWorkloadIdentity => {
                delete_workload_identity(ctx, region, physical_resource_id).await
            }
            // The delete addresses the alarm by the name it was created under, which is this
            // resource's physical id.
            Self::CloudWatchAlarm => {
                delete_metric_alarm(ctx, region, physical_resource_id).await
            }
            // The delete addresses the service by its own name inside the cluster it runs
            // on, which is what the ARN that is this resource's physical id carries.
            Self::EcsService => Box::pin(delete_ecs_service(ctx, region, resource)).await,
            // The delete addresses the spending limit by the ARN Amazon Braket minted for
            // it, which is this resource's physical id.
            Self::BraketSpendingLimit => {
                delete_braket_spending_limit(ctx, region, physical_resource_id).await
            }
            // The delete addresses the entity by the id Amplify UI Builder minted for it,
            // which is this resource's physical id, inside the app environment its declaration
            // names.
            Self::AmplifyUiBuilderComponent => {
                delete_ui_entity(ctx, region, UiEntity::Component, resource).await
            }
            Self::AmplifyUiBuilderForm => {
                delete_ui_entity(ctx, region, UiEntity::Form, resource).await
            }
            Self::AmplifyUiBuilderTheme => {
                delete_ui_entity(ctx, region, UiEntity::Theme, resource).await
            }
            // The delete addresses the app by the id AWS Amplify minted for it, which is the
            // id inside the ARN that is this resource's physical id.
            Self::AmplifyApp => delete_amplify_app(ctx, region, resource).await,
            // The delete addresses the environment by the name Amazon MWAA keyed it by,
            // which is this resource's physical id.
            Self::MwaaEnvironment => {
                delete_mwaa_environment(ctx, region, physical_resource_id).await
            }
            // A rule is addressed by its name or by the ARN X-Ray minted — "either name
            // or ARN, but not both" — and the ARN is this resource's physical id.
            Self::XraySamplingRule => delete_sampling_rule(ctx, region, resource).await,
            // Each of the three AWS Config types is addressed by its own name, which is
            // this resource's physical id.
            Self::ConfigConfigurationRecorder => {
                delete_configuration_recorder(ctx, region, physical_resource_id).await
            }
            Self::ConfigDeliveryChannel => {
                delete_delivery_channel(ctx, region, physical_resource_id).await
            }
            Self::ConfigConfigRule => {
                delete_config_rule(ctx, region, physical_resource_id).await
            }
        }
    }

    /// What the service that OWNS a resource reports the resource's configuration to be
    /// NOW: the document its own read answers with. This is the ACTUAL configuration
    /// [`super::drift`] compares a resource's expected template configuration against, so
    /// drift is read off the owning service rather than off CloudFormation's own record of
    /// what it once applied.
    ///
    /// `Ok(None)` is that service answering that it no longer holds the resource — judged
    /// on its own not-found CODE, never on a message, exactly as [`Self::delete`] does —
    /// which is the `DELETED` drift status. `Err(refusal)` is any other refusal: the check
    /// did not run, and the detection reports that rather than a resource as checked.
    pub(super) async fn actual_configuration(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        region: &str,
        resource: &StoredResource,
    ) -> Result<Option<Value>, Refusal> {
        let physical_resource_id = &resource.physical_resource_id;
        match self {
            Self::FirehoseDeliveryStream => {
                describe_delivery_stream(ctx, region, physical_resource_id).await
            }
            Self::KinesisStream => {
                describe_kinesis_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchMetricStream => {
                describe_metric_stream(ctx, region, physical_resource_id).await
            }
            Self::CloudWatchDashboard => {
                describe_dashboard(ctx, region, physical_resource_id).await
            }
            // CloudWatch's own read of the alarm the stack created, by the name it was
            // created under.
            Self::CloudWatchAlarm => {
                describe_metric_alarm(ctx, region, physical_resource_id).await
            }
            // Amazon ECS's own read of the service the stack created, by its name inside the
            // cluster it runs on.
            Self::EcsService => Box::pin(describe_ecs_service(ctx, region, resource)).await,
            Self::EcrPublicRepository => {
                describe_public_repository(ctx, region, physical_resource_id).await
            }
            Self::EcrRepository => describe_repository(ctx, region, physical_resource_id).await,
            Self::Ec2Vpc => describe_ec2_vpc(ctx, region, physical_resource_id).await,
            Self::Ec2Subnet => describe_ec2_subnet(ctx, region, physical_resource_id).await,
            Self::Ec2SecurityGroup => {
                describe_ec2_security_group(ctx, region, physical_resource_id).await
            }
            Self::Ec2Volume => describe_ec2_volume(ctx, region, physical_resource_id).await,
            Self::Ec2LaunchTemplate => {
                describe_ec2_launch_template(ctx, region, physical_resource_id).await
            }
            Self::AutoScalingGroup => {
                describe_auto_scaling_group(ctx, region, physical_resource_id).await
            }
            Self::S3Bucket => describe_bucket(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the document the named bucket carries
            // now, read back from Amazon S3.
            Self::S3BucketPolicy => describe_bucket_policy(ctx, region, resource).await,
            Self::S3TablesTableBucket => {
                describe_table_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsVectorBucket => {
                describe_vector_bucket(ctx, region, physical_resource_id).await
            }
            Self::S3VectorsIndex => describe_index(ctx, region, physical_resource_id).await,
            Self::EmrCluster => describe_job_flow(ctx, region, physical_resource_id).await,
            Self::IamRole => describe_role(ctx, region, physical_resource_id).await,
            Self::IamInstanceProfile => {
                describe_instance_profile(ctx, region, physical_resource_id).await
            }
            Self::IamUser => describe_iam_user(ctx, region, physical_resource_id).await,
            // Read back from the user's own list of keys, IAM publishing no read of ONE
            // access key.
            Self::IamAccessKey => describe_iam_access_key(ctx, region, resource).await,
            Self::IamManagedPolicy => {
                describe_managed_policy(ctx, region, physical_resource_id).await
            }
            Self::CodePipelinePipeline => {
                describe_pipeline(ctx, region, physical_resource_id).await
            }
            // Read back by the same triple the delete addresses the registration by.
            Self::CodePipelineCustomActionType => {
                describe_custom_action_type(ctx, region, resource).await
            }
            Self::EcsCluster => describe_cluster(ctx, region, physical_resource_id).await,
            Self::SqsQueue => describe_queue(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the `Policy` attribute of the queues it
            // names, which is read back from Amazon SQS as the document those queues carry
            // now.
            Self::SqsQueuePolicy => describe_queue_policy(ctx, region, resource).await,
            Self::OpenSearchServerlessCollection => {
                describe_collection(ctx, region, physical_resource_id).await
            }
            Self::OpenSearchServerlessSecurityPolicy => {
                describe_security_policy(ctx, region, resource).await
            }
            Self::OpenSearchServiceDomain => {
                describe_opensearch_domain(ctx, region, physical_resource_id).await
            }
            Self::SageMakerModelPackageGroup => {
                describe_model_package_group(ctx, region, physical_resource_id).await
            }
            // SageMaker's own reads of the three resources, each by the name it was created
            // under, reported under the property names the type spells its members with.
            Self::SageMakerModel => {
                describe_sagemaker_model(ctx, region, physical_resource_id).await
            }
            Self::SageMakerNotebookInstance => {
                describe_sagemaker_notebook_instance(ctx, region, physical_resource_id).await
            }
            Self::SageMakerNotebookInstanceLifecycleConfig => {
                describe_sagemaker_notebook_instance_lifecycle_config(
                    ctx,
                    region,
                    physical_resource_id,
                )
                .await
            }
            Self::SnsTopic => describe_topic(ctx, region, physical_resource_id).await,
            // A policy resource's configuration is the `Policy` attribute of the topics it
            // names, which is read back from Amazon SNS as the document those topics carry
            // now.
            Self::SnsTopicPolicy => describe_topic_policy(ctx, region, resource).await,
            Self::SnsSubscription => describe_subscription(ctx, region, resource).await,
            Self::FisExperimentTemplate => {
                describe_experiment_template(ctx, region, physical_resource_id).await
            }
            Self::WafRegionalIpSet => describe_ip_set(ctx, region, physical_resource_id).await,
            // Read back by the same triple the delete addresses the entity by.
            Self::Wafv2WebAcl => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_WEB_ACL_CALLS).await
            }
            Self::Wafv2RuleGroup => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_RULE_GROUP_CALLS).await
            }
            Self::Wafv2IpSet => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_IP_SET_CALLS).await
            }
            Self::Wafv2RegexPatternSet => {
                describe_wafv2_entity(ctx, region, resource, WAFV2_REGEX_PATTERN_SET_CALLS).await
            }
            // Read back as the web ACL AWS WAF reports in front of the protected resource
            // now, which is what the declared pair is compared against.
            Self::Wafv2WebAclAssociation => {
                describe_web_acl_association(ctx, region, resource).await
            }
            Self::GlueJob => describe_glue_job(ctx, region, physical_resource_id).await,
            // Read back by the same handles the deletes address the two catalog entities by,
            // and reported as the type's own properties: a database's definition is the
            // `DatabaseInput` property, a table's the `TableInput` one.
            Self::GlueDatabase => describe_glue_database(ctx, region, resource).await,
            Self::GlueTable => describe_glue_table(ctx, region, resource).await,
            Self::GlueCrawler => describe_glue_crawler(ctx, region, physical_resource_id).await,
            Self::GrafanaWorkspace => {
                describe_workspace(ctx, region, physical_resource_id).await
            }
            Self::GuardDutyDetector => describe_detector(ctx, region, physical_resource_id).await,
            // Read back by the same pair the delete addresses the list by.
            Self::GuardDutyIpSet => describe_guardduty_ip_set(ctx, region, resource).await,
            Self::KmsKey => describe_kms_key(ctx, region, physical_resource_id).await,
            Self::KmsAlias => describe_kms_alias(ctx, region, physical_resource_id).await,
            // One `DescribeClusterV2` reads a cluster of either kind back.
            Self::MskCluster | Self::MskServerlessCluster => {
                describe_msk_cluster(ctx, region, physical_resource_id).await
            }
            Self::MqBroker => describe_mq_broker(ctx, region, physical_resource_id).await,
            Self::RdsDbInstance => {
                describe_db_instance(ctx, region, physical_resource_id).await
            }
            Self::RdsDbSubnetGroup => {
                describe_db_subnet_group(ctx, region, physical_resource_id).await
            }
            Self::RdsDbParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            Self::RdsDbClusterParameterGroup => {
                describe_parameter_group(
                    ctx,
                    region,
                    physical_resource_id,
                    DB_CLUSTER_PARAMETER_GROUP_CALLS,
                )
                .await
            }
            // Amazon RDS's own read of the proxy, by the name it was created under.
            Self::RdsDbProxy => read_db_proxy(ctx, region, physical_resource_id).await,
            // Read back by the same proxy-and-group pair the delete addresses the
            // registrations by, which only the resource's own declaration carries.
            Self::RdsDbProxyTargetGroup => {
                describe_db_proxy_target_group(ctx, region, resource).await
            }
            // The read is addressed by the rule's name AND the bus it is bound to, which
            // only the resource's own record carries.
            Self::EventsRule => describe_rule(ctx, region, resource).await,
            Self::SsmParameter => describe_parameter(ctx, region, physical_resource_id).await,
            // Systems Manager's own read of the association the stack created, by the id it
            // minted for it.
            Self::SsmAssociation => {
                describe_ssm_association(ctx, region, physical_resource_id).await
            }
            Self::StepFunctionsActivity => {
                describe_activity(ctx, region, physical_resource_id).await
            }
            Self::SchedulerScheduleGroup => {
                describe_schedule_group(ctx, region, physical_resource_id).await
            }
            // Resource Groups' own read of the group, by the name it was created under.
            Self::ResourceGroupsGroup => {
                describe_resource_group(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the schedule's name AND its group, which only the
            // resource's own record carries.
            Self::SchedulerSchedule => describe_schedule(ctx, region, resource).await,
            Self::PipesPipe => {
                event_pipes::describe_pipe(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the group's name AND the pool that holds it, which
            // only the resource's own record carries.
            Self::CognitoUserPoolGroup => describe_group(ctx, region, resource).await,
            Self::AcmAcmeEndpoint => {
                describe_acme_endpoint(ctx, region, physical_resource_id).await
            }
            Self::AcmCertificate => describe_certificate(ctx, region, physical_resource_id).await,
            Self::AcmPcaCertificateAuthority => {
                describe_certificate_authority(ctx, region, physical_resource_id).await
            }
            // None of the properties an `AWS::ACMPCA::Certificate` declares — the
            // certification request it had signed, the template, the validity — is reported
            // by any read Amazon Web Services Private CA publishes for an issued
            // certificate, so what the check can establish about one is whether that
            // service still holds it.
            Self::AcmPcaCertificate => {
                issued_certificate_configuration(ctx, region, physical_resource_id).await
            }
            // An activation's one declared property that is also READ back is the status it
            // left the CA in, which `DescribeCertificateAuthority` reports.
            Self::AcmPcaCertificateAuthorityActivation => {
                activation_configuration(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverEndpoint => {
                describe_resolver_endpoint(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverRule => {
                describe_resolver_rule(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverRuleAssociation => {
                describe_resolver_rule_association(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallDomainList => {
                describe_firewall_domain_list(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallRuleGroup => {
                describe_firewall_rule_group(ctx, region, physical_resource_id).await
            }
            Self::Route53ResolverFirewallRuleGroupAssociation => {
                describe_firewall_rule_group_association(ctx, region, physical_resource_id).await
            }
            Self::Route53HostedZone => {
                describe_hosted_zone(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the record's own name, type and set identifier in the
            // zone its declaration names, which only the resource's own record carries.
            Self::Route53RecordSet => describe_record_set(ctx, region, resource).await,
            // Each record set the group declares, read back by its own name and type in the
            // zone the group names.
            Self::Route53RecordSetGroup => {
                describe_record_set_group(ctx, region, resource).await
            }
            // Read back by the id Route 53 minted, which is this resource's physical id.
            Self::Route53HealthCheck => describe_health_check(ctx, region, resource).await,
            // Read back by the identifier Route 53 minted for each, which is that
            // resource's physical id.
            Self::Route53CidrCollection => {
                describe_cidr_collection(ctx, region, physical_resource_id).await
            }
            Self::Route53DelegationSet => {
                describe_delegation_set(ctx, region, physical_resource_id).await
            }
            Self::Route53TrafficPolicy => {
                describe_traffic_policy(ctx, region, physical_resource_id).await
            }
            Self::ElasticBeanstalkApplication => {
                describe_eb_application(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the version's label AND the application that
            // holds it, which only the resource's own record carries.
            Self::ElasticBeanstalkApplicationVersion => {
                describe_eb_application_version(ctx, region, resource).await
            }
            Self::ElasticBeanstalkEnvironment => {
                describe_eb_environment(ctx, region, physical_resource_id).await
            }
            // Read back by the assigned `ServerId`, like the delete.
            Self::TransferServer => describe_transfer_server(ctx, region, resource).await,
            Self::CodeDeployApplication => {
                describe_codedeploy_application(ctx, region, physical_resource_id).await
            }
            // The read is addressed by the group's name AND the application that holds it,
            // which only the resource's own record carries.
            Self::CodeDeployDeploymentGroup => {
                describe_codedeploy_deployment_group(ctx, region, resource).await
            }
            Self::CodeDeployDeploymentConfig => {
                describe_codedeploy_deployment_config(ctx, region, physical_resource_id).await
            }
            // Batch's readers take a LIST and answer the records they found, so "no longer
            // held" is an empty list rather than a modelled not-found code
            // ([`batch_described`]).
            Self::BatchComputeEnvironment => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI,
                    "computeEnvironments",
                    physical_resource_id,
                )
                .await
            }
            Self::BatchJobQueue => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_JOB_QUEUES_URI,
                    "jobQueues",
                    physical_resource_id,
                )
                .await
            }
            Self::BatchServiceEnvironment => {
                batch_described(
                    ctx,
                    region,
                    BATCH_DESCRIBE_SERVICE_ENVIRONMENTS_URI,
                    "serviceEnvironments",
                    physical_resource_id,
                )
                .await
            }
            // Read back by the same cluster-and-principal pair the delete uses.
            Self::EksAccessEntry => describe_access_entry(ctx, region, resource).await,
            Self::EksCluster => describe_eks_cluster(ctx, region, physical_resource_id).await,
            Self::EksPodIdentityAssociation => {
                describe_pod_identity_association(ctx, region, resource).await
            }
            Self::ApiGatewayDomainName => {
                describe_apigateway_domain_name(ctx, region, physical_resource_id).await
            }
            // Read back by the same domain-and-base-path pair the delete uses.
            Self::ApiGatewayBasePathMapping => {
                describe_apigateway_base_path_mapping(ctx, region, resource).await
            }
            Self::ApiGatewayV2Api => {
                describe_apigatewayv2_api(ctx, region, physical_resource_id).await
            }
            Self::ApiGatewayV2DomainName => {
                describe_apigatewayv2_domain_name(ctx, region, physical_resource_id).await
            }
            // Read back by the same domain-and-id pair the delete uses.
            Self::ApiGatewayV2ApiMapping => {
                describe_apigatewayv2_api_mapping(ctx, region, resource).await
            }
            // Read back by the same api-and-identifier pair each delete uses.
            Self::ApiGatewayV2Integration => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_INTEGRATION_CHILD)
                    .await
            }
            Self::ApiGatewayV2Route => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_ROUTE_CHILD).await
            }
            Self::ApiGatewayV2Stage => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_STAGE_CHILD).await
            }
            Self::ApiGatewayV2Deployment => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_DEPLOYMENT_CHILD)
                    .await
            }
            Self::ApiGatewayV2Authorizer => {
                describe_apigatewayv2_child(ctx, region, resource, &APIGATEWAYV2_AUTHORIZER_CHILD)
                    .await
            }
            Self::CodeCommitRepository => {
                describe_codecommit_repository(ctx, region, physical_resource_id).await
            }
            Self::CodeArtifactDomain => describe_codeartifact_domain(ctx, region, resource).await,
            Self::CodeArtifactRepository => {
                describe_codeartifact_repository(ctx, region, resource).await
            }
            // Read back by the same ARN the delete addresses the group by.
            Self::CodeBuildReportGroup => {
                describe_codebuild_report_group(ctx, region, resource).await
            }
            // Read back out of the batch read of projects, by the name the project was
            // created under.
            Self::CodeBuildProject => {
                describe_codebuild_project(ctx, region, physical_resource_id).await
            }
            Self::SesConfigurationSet => {
                describe_ses_configuration_set(ctx, region, physical_resource_id).await
            }
            Self::SesReceiptRuleSet => {
                describe_ses_receipt_rule_set(ctx, region, physical_resource_id).await
            }
            // Read back by the same set-and-rule pair the delete addresses the rule by.
            Self::SesReceiptRule => describe_ses_receipt_rule(ctx, region, resource).await,
            Self::BackupVault => {
                describe_backup_vault(ctx, region, physical_resource_id).await
            }
            Self::BackupPlan => describe_backup_plan(ctx, region, physical_resource_id).await,
            // Read back by the same plan-and-selection pair the delete uses.
            Self::BackupSelection => describe_backup_selection(ctx, region, resource).await,
            // Amazon Redshift's own read of the cluster the stack created, by the
            // identifier it was created under.
            Self::RedshiftCluster => {
                describe_redshift_cluster(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock's own read of the guardrail the stack created, by the
            // identifier it was created under.
            Self::BedrockGuardrail => {
                describe_bedrock_guardrail(ctx, region, physical_resource_id).await
            }
            // Amazon Bedrock Agent's own read of the knowledge base the stack created, by
            // the id it minted for it.
            Self::BedrockKnowledgeBase => {
                describe_bedrock_knowledge_base(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift's own read of the parameter group the stack created, by the
            // name it was created under.
            Self::RedshiftClusterParameterGroup => {
                describe_cluster_parameter_group(ctx, region, physical_resource_id).await
            }
            // Amazon Redshift's own read of the subnet group the stack created, by the name
            // it was created under.
            Self::RedshiftClusterSubnetGroup => {
                describe_cluster_subnet_group(ctx, region, physical_resource_id).await
            }
            // The AgentCore control plane's own read of the identity, by the name it was
            // created under.
            Self::BedrockAgentCoreWorkloadIdentity => {
                describe_workload_identity(ctx, region, physical_resource_id).await
            }
            // Amazon Braket publishes no read of ONE spending limit, so the limit is read
            // out of the account's own listing, by the ARN that service minted for it.
            Self::BraketSpendingLimit => {
                describe_braket_spending_limit(ctx, region, physical_resource_id).await
            }
            // Amplify UI Builder's own read of the entity, by the id it minted, inside the app
            // environment the declaration names.
            Self::AmplifyUiBuilderComponent => {
                describe_ui_entity(ctx, region, UiEntity::Component, resource).await
            }
            Self::AmplifyUiBuilderForm => {
                describe_ui_entity(ctx, region, UiEntity::Form, resource).await
            }
            Self::AmplifyUiBuilderTheme => {
                describe_ui_entity(ctx, region, UiEntity::Theme, resource).await
            }
            // AWS Amplify's own read of the app, by the id it minted for it.
            Self::AmplifyApp => describe_amplify_app(ctx, region, resource).await,
            // Amazon MWAA's own read of the environment, by the name it was created under.
            Self::MwaaEnvironment => {
                describe_mwaa_environment(ctx, region, physical_resource_id).await
            }
            // X-Ray publishes no read of ONE sampling rule, so the rule is read out of the
            // account's own listing, by the identity that service minted for it.
            Self::XraySamplingRule => describe_sampling_rule(ctx, region, resource).await,
            // AWS Config's own reads of the three resources, each by the name it was
            // created under.
            Self::ConfigConfigurationRecorder => {
                describe_configuration_recorder(ctx, region, physical_resource_id).await
            }
            Self::ConfigDeliveryChannel => {
                describe_delivery_channel(ctx, region, physical_resource_id).await
            }
            Self::ConfigConfigRule => {
                describe_config_rule(ctx, region, physical_resource_id).await
            }
            // Amazon ElastiCache's own read of the cluster, by the identifier it was created
            // under. The cluster is reported with its SCALAR configuration — the engine, the
            // node type and count, the windows and the group names, each the member of the
            // same name the type declares — plus its configuration endpoint; a property that
            // is a LIST on the wire (the security groups, the log delivery configurations)
            // is left out, which is the position every arm whose property is not a scalar is
            // in and what [`super::drift`] documents.
            Self::ElastiCacheCacheCluster => {
                Box::pin(read_cache_cluster(ctx, region, physical_resource_id)).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Amazon Data Firehose: AWS::KinesisFirehose::DeliveryStream
// ---------------------------------------------------------------------------

#[cfg(test)]
// The per-type provisioning arms, grouped by the services that own the resources: each file
// holds the create, update, delete, read and drift arms of one group of types, and the enum
// above dispatches to them. Re-exported into this module, which is the one path every caller
// of an arm uses.
mod tests;
mod config_recording;
/// The Amazon CloudWatch alarm arms: visible to the rest of this service's state because
/// the type's own NAME lives here, beside the arms that provision it.
pub(in crate::state) mod cloudwatch_alarms;
/// The Amazon ECS SERVICE arms, beside the cluster arms [`application_services`] holds for
/// the same owning service.
mod ecs_services;
mod event_pipes;
mod bedrock_guardrails;
mod bedrock_knowledge_bases;
/// The Amazon SageMaker AI types a stack declares beside the model registry's: the model,
/// the notebook instance and the lifecycle configuration one of those runs. Visible to the
/// rest of this service's state because the type NAMES it owns are what the naming rules and
/// the return-value catalogue key their entries for those types by.
pub(crate) mod sagemaker_resources;
mod braket_spending_limits;
mod amplify_ui_builder;
mod amplify_apps;
mod mwaa_environments;
mod data_and_storage;
mod iam_identities;
mod application_services;
mod platform_services;
mod route53_policies;
/// The FLEET types a stack launches instances with — the Amazon EC2 launch template and the
/// Auto Scaling group that launches from one: visible to the rest of this service's state
/// because the INSTANCES a group holds are read there too, where the signals a resource's
/// `CreationPolicy` waits for are accounted for
/// ([`super::provisioning::instance_signals`]).
pub(super) mod compute_fleet;
mod network_and_delivery;
/// The Amazon S3 Tables arms: the table bucket a stack creates in that service, which is
/// keyed by its own ARN rather than by a name.
mod s3_table_buckets;
mod ssm_associations;
mod resource_groups;
pub(super) mod elasticache_caches;
pub(super) mod redshift_parameter_groups;
use elasticache_caches::*;
use redshift_parameter_groups::*;
use ssm_associations::*;
use resource_groups::*;
use compute_fleet::*;
pub(crate) use network_and_delivery::*;
use s3_table_buckets::*;
use platform_services::*;
use route53_policies::*;
pub(crate) use application_services::*;
use cloudwatch_alarms::*;
use ecs_services::*;
use config_recording::*;
use bedrock_guardrails::*;
use bedrock_knowledge_bases::*;
use sagemaker_resources::*;
use braket_spending_limits::*;
use amplify_ui_builder::*;
use amplify_apps::*;
use mwaa_environments::*;
use iam_identities::*;
use data_and_storage::*;

/// Amazon CloudWatch's wire identity, for the ROLLBACK TRIGGER monitoring beside this
/// catalogue ([`super::rollback_triggers`]): that module reads a trigger alarm's state out of
/// the same service this catalogue's `AWS::CloudWatch::*` arms write to, so the signing name
/// and the target prefix are read in ONE place and the two cannot drift apart.
pub(in crate::state) use data_and_storage::{CLOUDWATCH_SIGNING_NAME, CLOUDWATCH_TARGET_PREFIX};
