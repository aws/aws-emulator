// SPDX-License-Identifier: CC0-1.0

//! The BLUE/GREEN CONFIGURATION a deployment group carries — and the precondition
//! it does NOT carry, which is why no operation of this service consults one.
//!
//! `blueGreenDeploymentConfiguration` is "information about blue/green deployment
//! options for a deployment group"
//! (`smithy_query(query="shape",shape="BlueGreenDeploymentConfiguration")`) and
//! every option it holds reads as an option of a BLUE/GREEN deployment: when newly
//! provisioned instances start receiving traffic (`deploymentReadyOption`), how the
//! replacement environment is provisioned (`greenFleetProvisioningOption`), and what
//! becomes of the original environment once the deployment succeeds
//! (`terminateBlueInstancesOnDeploymentSuccess`). Whether a group runs blue/green
//! deployments is what the OTHER member says — `deploymentStyle`, over
//! `DeploymentType` (IN_PLACE | BLUE_GREEN) and `DeploymentOption`
//! (WITH_TRAFFIC_CONTROL | WITHOUT_TRAFFIC_CONTROL) — so the two members can plainly
//! disagree, and it is tempting to read that disagreement as a request CodeDeploy
//! would refuse with the modelled
//! `InvalidBlueGreenDeploymentConfigurationException` it holds for a blue/green
//! configuration "provided in an invalid format".
//!
//! MEASURED BEHAVIOUR SAYS OTHERWISE. A `CreateDeploymentGroup` is served, and the
//! group created, when it carries a blue/green configuration beside a deployment
//! type of IN_PLACE, beside a deployment option of WITHOUT_TRAFFIC_CONTROL, and when
//! a blue/green group carries no such configuration at all. The two members carry no
//! cross-precondition: the configuration is stored as given and read back as given,
//! and the deployment style decides which of those options a deployment of the group
//! acts on. What a group's style IS still judged against is its compute platform and
//! its own pair ([`super::deployment_style_gate`]), and its load balancer against
//! what that style requires ([`super::load_balancer_gate`]).
//!
//! The behaviour this module records is pinned, over the wire, by
//! `tests/all/blue_green_configuration_precondition.rs`, so a later change cannot
//! reintroduce a refusal CodeDeploy does not give.
