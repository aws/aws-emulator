Issue-Source: trace-sweep-divergence
Issue-Trace: ram-principal-association-workflow
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-principal-association-workflow ram-promotion-error-paths ram-resource-policies-lookup-error ram-resource-share-permission-association ram-resource-share-tagging ram-resource-share-lifecycle ram-catalog-list-permissions
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|CreateResourceShare|2xx->2xx|$.resourceShare.resourceShareConfiguration.exclusiveAccountAccess
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-promote-1c65f4a8"].resourceShareConfiguration.exclusiveAccountAccess
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-b0397a87"].resourceShareConfiguration.exclusiveAccountAccess
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-b0397a87-renamed"].resourceShareConfiguration.exclusiveAccountAccess
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-tags-d9722159"].resourceShareConfiguration.exclusiveAccountAccess
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/APIGatewayPrivateDomainNameManagementPolicyDefaultPermission"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMBlankEndEntityCertificateAPICSRPassthroughIssuanceCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMBlankEndEntityCertificateAPIPassthroughIssuanceCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionAPIGatewayDeveloperPortalProduct"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionAgentRegistryReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionAmazonDataZoneDomain"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionAppMesh"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionAppMeshPreview"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCapacityReservation"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCodeBuildProject"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCodeBuildReportGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionCoipPool"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionDedicatedHost"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionEBSVolume"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionFsx"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionGlueCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionGlueDatabase"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionGlueTable"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderComponent"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderContainerRecipe"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderImage"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderImageRecipe"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionLicenseConfiguration"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionLocalGateway"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallContainerAssociation"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallPolicy"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallStatefulRuleGroupWithDescribe"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallStatelessRuleGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionNetworkFirewallStatelessRuleGroupWithDescribe"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionODBCloudExadataInfrastructure"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionODBExascaleDbStorageVault"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionODBNetwork"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionOutpostsOutpost"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionOutpostsSite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionPrefixList"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionRDSCluster"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionRefactorSpacesEnvironment"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionResolverFirewallRuleGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionResolverQueryLogConfig"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionResolverRule"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionResourceGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionS3Outposts"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSSMContactsContact"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSSMIncidentsResponsePlan"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSSMParameterReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSageMakerModelPackageGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSageMakerPipeline"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionServerManageability"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionServiceDiscoveryNamespace"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionSubnet"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionTrafficMirror"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionTransitGateway"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionTransitGatewayMulticastDomain"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionsIpamPool"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionsPlacementGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionsSageMakerLineageGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionsSecurityGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEndEntityClientAuthCertificateIssuanceCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEndEntityServerAuthCertificateIssuanceCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEventBridgeEventBusV2EventSourceAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEventBridgeEventBusV2FullAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEventBridgeEventBusV2PublishOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMEventBridgeEventBusV2SubscribeOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMFSxRosettaPermissionPrefixList"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMLFEnabledGlueAllTablesReadWriteForDatabase"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMLFEnabledGlueDatabaseReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMLFEnabledGlueDatabaseReadWriteForTable"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMLFEnabledGlueTableReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAccessGrantsControl"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAccessGrantsData"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAccessGrantsReadAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAgentRegistryForAdmin"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAgentRegistryForConsumer"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAgentRegistryForPublisher"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAmazonDataZoneDomainFullAccessWithPortalAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAppSyncAllowSourceGraphQLAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAppSyncGraphQLApiInvokeAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAppSyncMergedApiOperationAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionAppSyncSourceApiOperationAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionBackupVaultReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionBedrockCustomModel"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionCloudHsmBackup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionCloudMapECSFullPermission"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionCodeConnectionsConnection"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionEBSVolumeCopyAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionElasticLoadBalancingTrustStore"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueAllTablesReadWriteForCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueAllTablesReadWriteForDatabase"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueDatabaseReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueDatabaseReadWriteForCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueDatabaseReadWriteForTable"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueTableReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueTableReadWriteForCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionGlueTableReadWriteForDatabase"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionIpamPoolByoipCidrImport"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionIpamResourceDiscovery"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueDatabaseReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueDatabaseReadWriteForCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueDatabaseReadWriteForTable"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueTableReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueTableReadWriteForCatalog"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionLFTagGlueTableReadWriteForDatabase"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionMlflowDefaultPolicy"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionNetworkFirewallFirewall"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionPartnerAppDefaultPolicy"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionResourceExplorerViews"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionRoute53ProfileAllowAssociation"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSMSVoiceOptOutList"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSMSVoicePhoneNumber"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSMSVoicePool"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSMSVoiceRCSAgent"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSMSVoiceSenderId"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSSMParameterReadOnlyWithHistory"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerCatalogResourceSearch"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerDataZoneProjectMembershipAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerFeatureGroupAdmin"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerFeatureGroupReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerFeatureGroupReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerModelCards"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerModelCardsAllowExport"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerModelPackageGroupAllowDeploy"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerModelPackageGroupAllowRegister"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSageMakerPipelineAllowExecution"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSagemakerHubFullAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSagemakerHubReadAndUse"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSagemakerHubReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionServiceCatalogAppRegistryApplicationAllowAssociation"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionServiceCatalogAppRegistryApplicationReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionServiceCatalogAppRegistryAttributeGroupAllowAssociation"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionServiceCatalogAppRegistryAttributeGroupReadOnly"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionSsmDocument"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionVerifiedAccessGroup"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionVpcLatticeResourceConfiguration"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionVpcLatticeServiceNetworkReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionVpcLatticeServiceReadWrite"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMPermissionsAmazonDatazoneDomainExtendedServiceWithPortalAccess"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMRevokeCertificateCertificateAuthority"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMSageMakerServicePrincipalPermissionAmazonDataZoneDomain"].restrictToServiceLinkedShares
Issue-Key: ram|ListPermissions|2xx->2xx|$.permissions[arn="arn:aws:ram::aws:permission/AWSRAMSubordinateCACertificatePathLen0IssuanceCertificateAuthority"].restrictToServiceLinkedShares
Issue-Commit: 8c779968f63825f959c68d9ef908c64428218fcd

# ram responses omit resource-share configuration and permission service-linked-share flags

> _Note: this issue was identified against the code base at commit `8c779968f63825f959c68d9ef908c64428218fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResourceShare

## Description
Every RAM read or write that projects a ResourceShare structure omits the `resourceShareConfiguration` member, and every read that projects a ResourceSharePermissionSummary omits the `restrictToServiceLinkedShares` member. AWS always emits both (with their boolean set to false when the feature is off); the emulator emits neither, so a client that reads the flag sees `None`/absent where AWS reports an explicit `false`. All HTTP statuses match (200 -> 200); the divergence is purely the serialized shape. 7 causes.

Cause 1 of 7 — CreateResourceShare's returned share has no resourceShareConfiguration (fixture ram-principal-association-workflow, step setup#1; the same cluster also covers ram-promotion-error-paths#setup#3, ram-resource-policies-lookup-error#setup#1, ram-resource-share-permission-association#setup#3, ram-resource-share-tagging#setup#1).
AWS: CreateResourceShare (Smithy: CreateResourceShareResponse.resourceShare -> ResourceShare; ResourceShare.resourceShareConfiguration -> ResourceShareConfiguration, doc 'The configuration of the resource share') returns the newly created share WITH its configuration sub-structure populated. The configuration's exclusive-account-access flag is recorded behavior only — the on-disk model's ResourceShareConfiguration declares only `retainSharingOnAccountLeaveOrganization`, so the member name comes from the recording, which shows `resourceShare.resourceShareConfiguration.exclusiveAccountAccess = false` on a share created without any resourceShareConfiguration in the request. The general rule: when the caller supplies no resourceShareConfiguration, AWS still returns the structure with its flags at their service defaults (false).
Emulator: HTTP 200 with a resourceShare that carries no resourceShareConfiguration member at all (expected false, got None).
Why: a client that reads share.resourceShareConfiguration.exclusiveAccountAccess to decide whether a share is restricted to the owning account dereferences an absent structure — a null-pointer/KeyError in hand-written clients, and in typed SDKs a None where AWS guarantees a concrete boolean. Code that round-trips a share (read, modify one field, write back) silently drops the configuration.
Fix: the CreateResourceShare handler's ResourceShare serializer — it must always emit resourceShareConfiguration with its flags defaulted to false — plus the resource-share state record, which has nowhere to hold the configuration.

Cause 2 of 7 — GetResourceShares omits resourceShareConfiguration.exclusiveAccountAccess on a share created for the promotion flow (fixture ram-promotion-error-paths, step task#4; same cluster also at ram-promotion-error-paths#task#7).
AWS: GetResourceShares (Smithy: GetResourceSharesResponse.resourceShares -> ResourceShareList of ResourceShare) projects the same ResourceShare structure as the create path, so each listed share carries resourceShareConfiguration with its flag explicitly false (recorded behavior only for the member name, as in cause 1).
Emulator: HTTP 200, the share is listed, but resourceShareConfiguration (and hence exclusiveAccountAccess) is absent.
Why: the read-back of a share never agrees with AWS's shape, so clients that compare a listed share against an expected document, or that filter shares on the exclusive-account-access flag, mis-classify every share.
Fix: the shared ResourceShare serializer used by GetResourceShares (same component as cause 1).

Cause 3 of 7 — GetResourceShares omits the same member on a freshly created share (fixture ram-resource-share-lifecycle, step task#2).
AWS: as cause 2 — the listed share carries resourceShareConfiguration with the flag false.
Emulator: member absent (expected false, got None).
Why: as cause 2.
Fix: as cause 2.

Cause 4 of 7 — GetResourceShares omits the same member after UpdateResourceShare renames the share (fixture ram-resource-share-lifecycle, step task#4; same cluster also at ram-resource-share-lifecycle#task#6).
AWS: a renamed share is still projected with resourceShareConfiguration present and its flag false; an update to name/allowExternalPrincipals does not remove the configuration.
Emulator: member still absent after the update.
Why: confirms the omission is in the serializer rather than in create-time state, and means the post-update read a lifecycle client performs can never match AWS.
Fix: as cause 2.

Cause 5 of 7 — GetResourceShares omits the same member on a tagged share (fixture ram-resource-share-tagging, step task#3; same cluster also at ram-resource-share-tagging#task#5).
AWS: as cause 2.
Emulator: member absent.
Why: as cause 2.
Fix: as cause 2.

Cause 6 of 7 — ListPermissions omits restrictToServiceLinkedShares on every AWS-managed permission in the catalog (fixture ram-catalog-list-permissions, step task#1).
AWS: ListPermissions (Smithy: ListPermissionsResponse.permissions -> ResourceSharePermissionList of ResourceSharePermissionSummary) returns one summary per managed permission. The `restrictToServiceLinkedShares` member is recorded behavior only — the on-disk ResourceSharePermissionSummary does not declare it — and the recording shows it present with value false on EVERY AWS-managed permission returned (the divergence lists it missing for the whole catalogue, from APIGatewayPrivateDomainNameManagementPolicyDefaultPermission through AWSRAMSubordinateCACertificatePathLen0IssuanceCertificateAuthority). The rule: a managed permission that is not restricted to service-linked shares reports the flag as an explicit false, not by omission.
Emulator: HTTP 200 with the permission summaries present but no restrictToServiceLinkedShares member on any of them (expected false, got None for every arn).
Why: a client deciding whether a managed permission may be attached to a customer-created share reads this flag; an absent member is indistinguishable from 'unknown', so permission-selection logic either crashes or treats restricted and unrestricted permissions alike.
Fix: the ResourceSharePermissionSummary serializer shared by ListPermissions (and the managed-permission catalogue records it reads from, which carry no such field).

Cause 7 of 7 — ListPermissions omits restrictToServiceLinkedShares when filtered to a single permission (fixture ram-resource-share-permission-association, step setup#2).
AWS: as cause 6 — the single returned summary for arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderComponent carries restrictToServiceLinkedShares = false.
Emulator: member absent.
Why: as cause 6; it also shows the omission is not an artefact of the large unfiltered catalogue page.
Fix: as cause 6.

Related open issues (neither covers these causes): 20261126T135339Z-ram-listresourcetypes-serves-an-emulator-authored-shareable-resource.md owns the ListResourceTypes catalogue content, and 20261119T221252Z-ram-managed-permission-reads-serve-wrong-permission-state.md owns permission status/version state — this issue is about two response members that are never serialized at all.

Traces: ram-principal-association-workflow#setup#1; ram-promotion-error-paths#task#4; ram-resource-share-lifecycle#task#2; ram-resource-share-lifecycle#task#4; ram-resource-share-tagging#task#3; ram-catalog-list-permissions#task#1; ram-resource-share-permission-association#setup#2.

## Evidence
Cause 1 — ram-principal-association-workflow#setup#1, CreateResourceShare, expected 200 / actual 200: $.resourceShare.resourceShareConfiguration.exclusiveAccountAccess missing (expected False, got None). Cluster also affects ram-promotion-error-paths#setup#3, ram-resource-policies-lookup-error#setup#1, ram-resource-share-permission-association#setup#3, ram-resource-share-tagging#setup#1 (affected_count 5).
Cause 2 — ram-promotion-error-paths#task#4, GetResourceShares, 200/200: $.resourceShares[name="<share-name>"].resourceShareConfiguration.exclusiveAccountAccess missing (expected False, got None); also ram-promotion-error-paths#task#7.
Cause 3 — ram-resource-share-lifecycle#task#2, GetResourceShares, 200/200: same member missing on the created share.
Cause 4 — ram-resource-share-lifecycle#task#4, GetResourceShares, 200/200: same member missing on the renamed share; also ram-resource-share-lifecycle#task#6.
Cause 5 — ram-resource-share-tagging#task#3, GetResourceShares, 200/200: same member missing; also ram-resource-share-tagging#task#5.
Cause 6 — ram-catalog-list-permissions#task#1, ListPermissions, 200/200: $.permissions[arn="..."].restrictToServiceLinkedShares missing (expected False, got None) for every managed permission in the response.
Cause 7 — ram-resource-share-permission-association#setup#2, ListPermissions, 200/200: $.permissions[arn="arn:aws:ram::aws:permission/AWSRAMDefaultPermissionImageBuilderComponent"].restrictToServiceLinkedShares missing (expected False, got None).
Note: share names and permission ARNs above are per-run values from the recording; the requirement is that the member is present with an explicit boolean, not the particular share name.

## Steps to Reproduce
1. POST /createresourceshare with a name and no resourceShareConfiguration -> 200, but the returned resourceShare has no resourceShareConfiguration member (AWS returns it with exclusiveAccountAccess=false). (ram-principal-association-workflow#setup#1)
2. POST /getresourceshares with resourceOwner=SELF -> 200; each listed share likewise has no resourceShareConfiguration. (ram-resource-share-lifecycle#task#2)
3. POST /updateresourceshare to rename the share, then repeat step 2 -> the member is still absent. (ram-resource-share-lifecycle#task#4)
4. POST /tagresource on the share, then repeat step 2 -> still absent. (ram-resource-share-tagging#task#3)
5. POST /listpermissions (unfiltered) -> 200; no permission summary carries restrictToServiceLinkedShares. (ram-catalog-list-permissions#task#1)
6. POST /listpermissions filtered to resourceType imagebuilder:Component -> the single summary for AWSRAMDefaultPermissionImageBuilderComponent also lacks restrictToServiceLinkedShares. (ram-resource-share-permission-association#setup#2)
