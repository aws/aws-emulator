// SPDX-License-Identifier: CC0-1.0

//! The IAM action name each operation of this service is authorized on.
//!
//! IAM authorizes its own API, and a policy statement's `Action` element names an action
//! by its `iam:<Operation>` spelling, so every operation handler needs that spelling to
//! ask [`super::authorize`] its question. They live here, in one module beside the gate
//! that consumes them, rather than one per handler: the gate decides every operation of
//! the service, so the set of names it decides is one list, and a handler that spelled
//! its own would be the one place a typo could silently widen a grant.
//!
//! Each name is the operation's own, as the Service Authorization Reference ("Actions,
//! resources, and condition keys for Identity And Access Management") spells it —
//! including the capitalisation IAM gives an initialism (`iam:ListOpenIDConnectProviders`,
//! `iam:TagSAMLProvider`, `iam:GetMFADevice`), which is not the camel case a mechanical
//! transform of the Rust name would produce.
//!
//! They are re-exported from [`super`], so a handler names one as
//! `crate::state::authorization::<CONST>` whichever module it is declared in.

/// `iam:ListGroups`.
pub const LIST_GROUPS: &str = "iam:ListGroups";

/// `iam:ListRoles`.
pub const LIST_ROLES: &str = "iam:ListRoles";

/// `iam:ListUsers`.
pub const LIST_USERS: &str = "iam:ListUsers";

/// `iam:ListPolicies`.
pub const LIST_POLICIES: &str = "iam:ListPolicies";

/// `iam:ListInstanceProfiles`.
pub const LIST_INSTANCE_PROFILES: &str = "iam:ListInstanceProfiles";

/// `iam:ListServerCertificates`.
pub const LIST_SERVER_CERTIFICATES: &str = "iam:ListServerCertificates";

/// `iam:ListInstanceProfilesForRole`.
pub const LIST_INSTANCE_PROFILES_FOR_ROLE: &str = "iam:ListInstanceProfilesForRole";

/// `iam:ListPoliciesGrantingServiceAccess`.
pub const LIST_POLICIES_GRANTING_SERVICE_ACCESS: &str = "iam:ListPoliciesGrantingServiceAccess";

/// `iam:GenerateServiceLastAccessedDetails`.
pub const GENERATE_SERVICE_LAST_ACCESSED_DETAILS: &str = "iam:GenerateServiceLastAccessedDetails";

/// `iam:GetServiceLastAccessedDetails`.
pub const GET_SERVICE_LAST_ACCESSED_DETAILS: &str = "iam:GetServiceLastAccessedDetails";

/// `iam:GetServiceLastAccessedDetailsWithEntities`.
pub const GET_SERVICE_LAST_ACCESSED_DETAILS_WITH_ENTITIES: &str =
    "iam:GetServiceLastAccessedDetailsWithEntities";

/// `iam:ListOpenIDConnectProviders`.
pub const LIST_OPEN_ID_CONNECT_PROVIDERS: &str = "iam:ListOpenIDConnectProviders";

/// `iam:CreateGroup`.
pub const CREATE_GROUP: &str = "iam:CreateGroup";

/// `iam:DeleteGroup`.
pub const DELETE_GROUP: &str = "iam:DeleteGroup";

/// `iam:GetGroup`.
pub const GET_GROUP: &str = "iam:GetGroup";

/// `iam:UpdateGroup`.
pub const UPDATE_GROUP: &str = "iam:UpdateGroup";

/// `iam:GetUser`.
pub const GET_USER: &str = "iam:GetUser";

/// `iam:UpdateUser`.
pub const UPDATE_USER: &str = "iam:UpdateUser";

/// `iam:UpdateServerCertificate`.
pub const UPDATE_SERVER_CERTIFICATE: &str = "iam:UpdateServerCertificate";

/// `iam:GetUserPolicy`.
pub const GET_USER_POLICY: &str = "iam:GetUserPolicy";

/// `iam:ListGroupsForUser`.
pub const LIST_GROUPS_FOR_USER: &str = "iam:ListGroupsForUser";

/// `iam:ListUserTags`.
pub const LIST_USER_TAGS: &str = "iam:ListUserTags";

/// `iam:ListUserPolicies`.
pub const LIST_USER_POLICIES: &str = "iam:ListUserPolicies";

/// `iam:ListAccessKeys`.
pub const LIST_ACCESS_KEYS: &str = "iam:ListAccessKeys";

/// `iam:ListRoleTags`.
pub const LIST_ROLE_TAGS: &str = "iam:ListRoleTags";

/// `iam:ListPolicyTags`.
pub const LIST_POLICY_TAGS: &str = "iam:ListPolicyTags";

/// `iam:ListInstanceProfileTags`.
pub const LIST_INSTANCE_PROFILE_TAGS: &str = "iam:ListInstanceProfileTags";

/// `iam:ListOpenIDConnectProviderTags`.
pub const LIST_OPEN_ID_CONNECT_PROVIDER_TAGS: &str = "iam:ListOpenIDConnectProviderTags";

/// `iam:TagPolicy`.
pub const TAG_POLICY: &str = "iam:TagPolicy";

/// `iam:CreateUser`.
pub const CREATE_USER: &str = "iam:CreateUser";

/// `iam:CreateRole`.
pub const CREATE_ROLE: &str = "iam:CreateRole";

/// `iam:CreatePolicy`.
pub const CREATE_POLICY: &str = "iam:CreatePolicy";

/// `iam:CreateInstanceProfile`.
pub const CREATE_INSTANCE_PROFILE: &str = "iam:CreateInstanceProfile";

/// `iam:TagUser`.
pub const TAG_USER: &str = "iam:TagUser";

/// `iam:UntagUser`.
pub const UNTAG_USER: &str = "iam:UntagUser";

/// `iam:TagRole`.
pub const TAG_ROLE: &str = "iam:TagRole";

/// `iam:UntagRole`.
pub const UNTAG_ROLE: &str = "iam:UntagRole";

/// `iam:TagInstanceProfile`.
pub const TAG_INSTANCE_PROFILE: &str = "iam:TagInstanceProfile";

/// `iam:UntagInstanceProfile`.
pub const UNTAG_INSTANCE_PROFILE: &str = "iam:UntagInstanceProfile";

/// `iam:TagOpenIDConnectProvider`.
pub const TAG_OPEN_ID_CONNECT_PROVIDER: &str = "iam:TagOpenIDConnectProvider";

/// `iam:UntagOpenIDConnectProvider`.
pub const UNTAG_OPEN_ID_CONNECT_PROVIDER: &str = "iam:UntagOpenIDConnectProvider";

/// `iam:TagServerCertificate`.
pub const TAG_SERVER_CERTIFICATE: &str = "iam:TagServerCertificate";

/// `iam:UntagServerCertificate`.
pub const UNTAG_SERVER_CERTIFICATE: &str = "iam:UntagServerCertificate";

/// `iam:TagSAMLProvider`.
pub const TAG_SAML_PROVIDER: &str = "iam:TagSAMLProvider";

/// `iam:UntagSAMLProvider`.
pub const UNTAG_SAML_PROVIDER: &str = "iam:UntagSAMLProvider";

/// `iam:UntagPolicy`.
pub const UNTAG_POLICY: &str = "iam:UntagPolicy";

/// `iam:CreatePolicyVersion`.
pub const CREATE_POLICY_VERSION: &str = "iam:CreatePolicyVersion";

/// `iam:DeletePolicyVersion`.
pub const DELETE_POLICY_VERSION: &str = "iam:DeletePolicyVersion";

/// `iam:SetDefaultPolicyVersion`.
pub const SET_DEFAULT_POLICY_VERSION: &str = "iam:SetDefaultPolicyVersion";

/// `iam:GetAccountAuthorizationDetails`.
pub const GET_ACCOUNT_AUTHORIZATION_DETAILS: &str = "iam:GetAccountAuthorizationDetails";

/// `iam:GetDelegationRequest`.
pub const GET_DELEGATION_REQUEST: &str = "iam:GetDelegationRequest";

/// `iam:CreateDelegationRequest`.
pub const CREATE_DELEGATION_REQUEST: &str = "iam:CreateDelegationRequest";

/// `iam:UpdateDelegationRequest`.
pub const UPDATE_DELEGATION_REQUEST: &str = "iam:UpdateDelegationRequest";

/// `iam:AcceptDelegationRequest`.
pub const ACCEPT_DELEGATION_REQUEST: &str = "iam:AcceptDelegationRequest";

/// `iam:RejectDelegationRequest`.
pub const REJECT_DELEGATION_REQUEST: &str = "iam:RejectDelegationRequest";

/// `iam:AssociateDelegationRequest`.
pub const ASSOCIATE_DELEGATION_REQUEST: &str = "iam:AssociateDelegationRequest";

/// `iam:AcquireRole`.
pub const ACQUIRE_ROLE: &str = "iam:AcquireRole";

/// `iam:GetRoleTemplateVersion`.
pub const GET_ROLE_TEMPLATE_VERSION: &str = "iam:GetRoleTemplateVersion";

/// `iam:GenerateCredentialReport`.
pub const GENERATE_CREDENTIAL_REPORT: &str = "iam:GenerateCredentialReport";

/// `iam:GetCredentialReport`.
pub const GET_CREDENTIAL_REPORT: &str = "iam:GetCredentialReport";

/// `iam:GenerateOrganizationsAccessReport`.
pub const GENERATE_ORGANIZATIONS_ACCESS_REPORT: &str = "iam:GenerateOrganizationsAccessReport";

/// `iam:ListOrganizationsFeatures`.
pub const LIST_ORGANIZATIONS_FEATURES: &str = "iam:ListOrganizationsFeatures";

/// `iam:ListDelegationRequests`.
pub const LIST_DELEGATION_REQUESTS: &str = "iam:ListDelegationRequests";

/// `iam:GetAccountProperties`.
pub const GET_ACCOUNT_PROPERTIES: &str = "iam:GetAccountProperties";

/// `iam:PutAccountProperties`.
pub const PUT_ACCOUNT_PROPERTIES: &str = "iam:PutAccountProperties";

/// `iam:UpdateAccountPasswordPolicy`.
pub const UPDATE_ACCOUNT_PASSWORD_POLICY: &str = "iam:UpdateAccountPasswordPolicy";

/// `iam:GetAccountPasswordPolicy`.
pub const GET_ACCOUNT_PASSWORD_POLICY: &str = "iam:GetAccountPasswordPolicy";

/// `iam:DeleteAccountPasswordPolicy`.
pub const DELETE_ACCOUNT_PASSWORD_POLICY: &str = "iam:DeleteAccountPasswordPolicy";

/// `iam:CreateAccountAlias`.
pub const CREATE_ACCOUNT_ALIAS: &str = "iam:CreateAccountAlias";

/// `iam:DeleteAccountAlias`.
pub const DELETE_ACCOUNT_ALIAS: &str = "iam:DeleteAccountAlias";

/// `iam:ListAccountAliases`.
pub const LIST_ACCOUNT_ALIASES: &str = "iam:ListAccountAliases";

/// `iam:ListAttachedUserPolicies`.
pub const LIST_ATTACHED_USER_POLICIES: &str = "iam:ListAttachedUserPolicies";

/// `iam:ListAttachedRolePolicies`.
pub const LIST_ATTACHED_ROLE_POLICIES: &str = "iam:ListAttachedRolePolicies";

/// `iam:ListAttachedGroupPolicies`.
pub const LIST_ATTACHED_GROUP_POLICIES: &str = "iam:ListAttachedGroupPolicies";

/// `iam:AttachUserPolicy`.
pub const ATTACH_USER_POLICY: &str = "iam:AttachUserPolicy";

/// `iam:AttachRolePolicy`.
pub const ATTACH_ROLE_POLICY: &str = "iam:AttachRolePolicy";

/// `iam:AttachGroupPolicy`.
pub const ATTACH_GROUP_POLICY: &str = "iam:AttachGroupPolicy";

/// `iam:DetachUserPolicy`.
pub const DETACH_USER_POLICY: &str = "iam:DetachUserPolicy";

/// `iam:DetachRolePolicy`.
pub const DETACH_ROLE_POLICY: &str = "iam:DetachRolePolicy";

/// `iam:DetachGroupPolicy`.
pub const DETACH_GROUP_POLICY: &str = "iam:DetachGroupPolicy";

/// `iam:PutUserPolicy`.
pub const PUT_USER_POLICY: &str = "iam:PutUserPolicy";

/// `iam:PutRolePolicy`.
pub const PUT_ROLE_POLICY: &str = "iam:PutRolePolicy";

/// `iam:PutGroupPolicy`.
pub const PUT_GROUP_POLICY: &str = "iam:PutGroupPolicy";

/// `iam:DeleteUserPolicy`.
pub const DELETE_USER_POLICY: &str = "iam:DeleteUserPolicy";

/// `iam:DeleteRolePolicy`.
pub const DELETE_ROLE_POLICY: &str = "iam:DeleteRolePolicy";

/// `iam:DeleteGroupPolicy`.
pub const DELETE_GROUP_POLICY: &str = "iam:DeleteGroupPolicy";

/// `iam:GetRolePolicy`.
pub const GET_ROLE_POLICY: &str = "iam:GetRolePolicy";

/// `iam:GetGroupPolicy`.
pub const GET_GROUP_POLICY: &str = "iam:GetGroupPolicy";

/// `iam:ListRolePolicies`.
pub const LIST_ROLE_POLICIES: &str = "iam:ListRolePolicies";

/// `iam:ListGroupPolicies`.
pub const LIST_GROUP_POLICIES: &str = "iam:ListGroupPolicies";

/// `iam:PutUserPermissionsBoundary`.
pub const PUT_USER_PERMISSIONS_BOUNDARY: &str = "iam:PutUserPermissionsBoundary";

/// `iam:PutRolePermissionsBoundary`.
pub const PUT_ROLE_PERMISSIONS_BOUNDARY: &str = "iam:PutRolePermissionsBoundary";

/// `iam:DeleteUserPermissionsBoundary`.
pub const DELETE_USER_PERMISSIONS_BOUNDARY: &str = "iam:DeleteUserPermissionsBoundary";

/// `iam:DeleteRolePermissionsBoundary`.
pub const DELETE_ROLE_PERMISSIONS_BOUNDARY: &str = "iam:DeleteRolePermissionsBoundary";

/// `iam:DeleteUser`.
pub const DELETE_USER: &str = "iam:DeleteUser";

/// `iam:DeleteRole`.
pub const DELETE_ROLE: &str = "iam:DeleteRole";

/// `iam:DeletePolicy`.
pub const DELETE_POLICY: &str = "iam:DeletePolicy";

/// `iam:GetPolicy`.
pub const GET_POLICY: &str = "iam:GetPolicy";

/// `iam:GetPolicyVersion`.
pub const GET_POLICY_VERSION: &str = "iam:GetPolicyVersion";

/// `iam:ListPolicyVersions`.
pub const LIST_POLICY_VERSIONS: &str = "iam:ListPolicyVersions";

/// `iam:ListEntitiesForPolicy`.
pub const LIST_ENTITIES_FOR_POLICY: &str = "iam:ListEntitiesForPolicy";

/// `iam:GetRole`.
pub const GET_ROLE: &str = "iam:GetRole";

/// `iam:UpdateRole`.
pub const UPDATE_ROLE: &str = "iam:UpdateRole";

/// `iam:UpdateRoleDescription`.
pub const UPDATE_ROLE_DESCRIPTION: &str = "iam:UpdateRoleDescription";

/// `iam:UpdateAssumeRolePolicy`.
pub const UPDATE_ASSUME_ROLE_POLICY: &str = "iam:UpdateAssumeRolePolicy";

/// `iam:CreateServiceLinkedRole`.
pub const CREATE_SERVICE_LINKED_ROLE: &str = "iam:CreateServiceLinkedRole";

/// `iam:DeleteServiceLinkedRole`.
pub const DELETE_SERVICE_LINKED_ROLE: &str = "iam:DeleteServiceLinkedRole";

/// `iam:GetServiceLinkedRoleDeletionStatus`.
pub const GET_SERVICE_LINKED_ROLE_DELETION_STATUS: &str = "iam:GetServiceLinkedRoleDeletionStatus";

/// `iam:AddUserToGroup`.
pub const ADD_USER_TO_GROUP: &str = "iam:AddUserToGroup";

/// `iam:RemoveUserFromGroup`.
pub const REMOVE_USER_FROM_GROUP: &str = "iam:RemoveUserFromGroup";

/// `iam:GetInstanceProfile`.
pub const GET_INSTANCE_PROFILE: &str = "iam:GetInstanceProfile";

/// `iam:DeleteInstanceProfile`.
pub const DELETE_INSTANCE_PROFILE: &str = "iam:DeleteInstanceProfile";

/// `iam:AddRoleToInstanceProfile`.
pub const ADD_ROLE_TO_INSTANCE_PROFILE: &str = "iam:AddRoleToInstanceProfile";

/// `iam:RemoveRoleFromInstanceProfile`.
pub const REMOVE_ROLE_FROM_INSTANCE_PROFILE: &str = "iam:RemoveRoleFromInstanceProfile";

/// `iam:UploadServerCertificate`.
pub const UPLOAD_SERVER_CERTIFICATE: &str = "iam:UploadServerCertificate";

/// `iam:GetServerCertificate`.
pub const GET_SERVER_CERTIFICATE: &str = "iam:GetServerCertificate";

/// `iam:DeleteServerCertificate`.
pub const DELETE_SERVER_CERTIFICATE: &str = "iam:DeleteServerCertificate";

/// `iam:ListServerCertificateTags`.
pub const LIST_SERVER_CERTIFICATE_TAGS: &str = "iam:ListServerCertificateTags";

/// `iam:CreateSAMLProvider`.
pub const CREATE_SAML_PROVIDER: &str = "iam:CreateSAMLProvider";

/// `iam:GetSAMLProvider`.
pub const GET_SAML_PROVIDER: &str = "iam:GetSAMLProvider";

/// `iam:UpdateSAMLProvider`.
pub const UPDATE_SAML_PROVIDER: &str = "iam:UpdateSAMLProvider";

/// `iam:DeleteSAMLProvider`.
pub const DELETE_SAML_PROVIDER: &str = "iam:DeleteSAMLProvider";

/// `iam:ListSAMLProviders`.
pub const LIST_SAML_PROVIDERS: &str = "iam:ListSAMLProviders";

/// `iam:ListSAMLProviderTags`.
pub const LIST_SAML_PROVIDER_TAGS: &str = "iam:ListSAMLProviderTags";

/// `iam:CreateOpenIDConnectProvider`.
pub const CREATE_OPEN_ID_CONNECT_PROVIDER: &str = "iam:CreateOpenIDConnectProvider";

/// `iam:GetOpenIDConnectProvider`.
pub const GET_OPEN_ID_CONNECT_PROVIDER: &str = "iam:GetOpenIDConnectProvider";

/// `iam:DeleteOpenIDConnectProvider`.
pub const DELETE_OPEN_ID_CONNECT_PROVIDER: &str = "iam:DeleteOpenIDConnectProvider";

/// `iam:AddClientIDToOpenIDConnectProvider`.
pub const ADD_CLIENT_ID_TO_OPEN_ID_CONNECT_PROVIDER: &str =
    "iam:AddClientIDToOpenIDConnectProvider";

/// `iam:RemoveClientIDFromOpenIDConnectProvider`.
pub const REMOVE_CLIENT_ID_FROM_OPEN_ID_CONNECT_PROVIDER: &str =
    "iam:RemoveClientIDFromOpenIDConnectProvider";

/// `iam:UpdateOpenIDConnectProviderThumbprint`.
pub const UPDATE_OPEN_ID_CONNECT_PROVIDER_THUMBPRINT: &str =
    "iam:UpdateOpenIDConnectProviderThumbprint";

/// `iam:CreateAccessKey`.
pub const CREATE_ACCESS_KEY: &str = "iam:CreateAccessKey";

/// `iam:UpdateAccessKey`.
pub const UPDATE_ACCESS_KEY: &str = "iam:UpdateAccessKey";

/// `iam:DeleteAccessKey`.
pub const DELETE_ACCESS_KEY: &str = "iam:DeleteAccessKey";

/// `iam:GetAccessKeyLastUsed`.
pub const GET_ACCESS_KEY_LAST_USED: &str = "iam:GetAccessKeyLastUsed";

/// `iam:CreateLoginProfile`.
pub const CREATE_LOGIN_PROFILE: &str = "iam:CreateLoginProfile";

/// `iam:GetLoginProfile`.
pub const GET_LOGIN_PROFILE: &str = "iam:GetLoginProfile";

/// `iam:UpdateLoginProfile`.
pub const UPDATE_LOGIN_PROFILE: &str = "iam:UpdateLoginProfile";

/// `iam:DeleteLoginProfile`.
pub const DELETE_LOGIN_PROFILE: &str = "iam:DeleteLoginProfile";

/// `iam:ChangePassword`.
pub const CHANGE_PASSWORD: &str = "iam:ChangePassword";

/// `iam:GetMFADevice`.
pub const GET_MFA_DEVICE: &str = "iam:GetMFADevice";

/// `iam:CreateVirtualMFADevice`.
pub const CREATE_VIRTUAL_MFA_DEVICE: &str = "iam:CreateVirtualMFADevice";

/// `iam:EnableMFADevice`.
pub const ENABLE_MFA_DEVICE: &str = "iam:EnableMFADevice";

/// `iam:ListVirtualMFADevices`.
pub const LIST_VIRTUAL_MFA_DEVICES: &str = "iam:ListVirtualMFADevices";

/// `iam:UploadSigningCertificate`.
pub const UPLOAD_SIGNING_CERTIFICATE: &str = "iam:UploadSigningCertificate";

/// `iam:UpdateSigningCertificate`.
pub const UPDATE_SIGNING_CERTIFICATE: &str = "iam:UpdateSigningCertificate";

/// `iam:DeleteSigningCertificate`.
pub const DELETE_SIGNING_CERTIFICATE: &str = "iam:DeleteSigningCertificate";

/// `iam:ListSigningCertificates`.
pub const LIST_SIGNING_CERTIFICATES: &str = "iam:ListSigningCertificates";

/// `iam:UploadSSHPublicKey`.
pub const UPLOAD_SSH_PUBLIC_KEY: &str = "iam:UploadSSHPublicKey";

/// `iam:ListSSHPublicKeys`.
pub const LIST_SSH_PUBLIC_KEYS: &str = "iam:ListSSHPublicKeys";

/// `iam:GetSSHPublicKey`.
pub const GET_SSH_PUBLIC_KEY: &str = "iam:GetSSHPublicKey";

/// `iam:CreateServiceSpecificCredential`.
pub const CREATE_SERVICE_SPECIFIC_CREDENTIAL: &str = "iam:CreateServiceSpecificCredential";

/// `iam:UpdateServiceSpecificCredential`.
pub const UPDATE_SERVICE_SPECIFIC_CREDENTIAL: &str = "iam:UpdateServiceSpecificCredential";

/// `iam:ResetServiceSpecificCredential`.
pub const RESET_SERVICE_SPECIFIC_CREDENTIAL: &str = "iam:ResetServiceSpecificCredential";

/// `iam:DeleteServiceSpecificCredential`.
pub const DELETE_SERVICE_SPECIFIC_CREDENTIAL: &str = "iam:DeleteServiceSpecificCredential";

/// `iam:ListServiceSpecificCredentials`.
pub const LIST_SERVICE_SPECIFIC_CREDENTIALS: &str = "iam:ListServiceSpecificCredentials";

/// `iam:GetContextKeysForCustomPolicy`.
pub const GET_CONTEXT_KEYS_FOR_CUSTOM_POLICY: &str = "iam:GetContextKeysForCustomPolicy";

/// `iam:GetContextKeysForPrincipalPolicy`.
pub const GET_CONTEXT_KEYS_FOR_PRINCIPAL_POLICY: &str = "iam:GetContextKeysForPrincipalPolicy";

/// `iam:SimulateCustomPolicy`.
pub const SIMULATE_CUSTOM_POLICY: &str = "iam:SimulateCustomPolicy";

/// `iam:SimulatePrincipalPolicy`.
pub const SIMULATE_PRINCIPAL_POLICY: &str = "iam:SimulatePrincipalPolicy";

/// `iam:GetOrganizationsAccessReport`.
pub const GET_ORGANIZATIONS_ACCESS_REPORT: &str = "iam:GetOrganizationsAccessReport";

/// `iam:GetHumanReadableSummary`.
pub const GET_HUMAN_READABLE_SUMMARY: &str = "iam:GetHumanReadableSummary";

/// `iam:EnableOutboundWebIdentityFederation`.
pub const ENABLE_OUTBOUND_WEB_IDENTITY_FEDERATION: &str =
    "iam:EnableOutboundWebIdentityFederation";

/// `iam:GetOutboundWebIdentityFederationInfo`.
pub const GET_OUTBOUND_WEB_IDENTITY_FEDERATION_INFO: &str =
    "iam:GetOutboundWebIdentityFederationInfo";

/// `iam:DisableOutboundWebIdentityFederation`.
pub const DISABLE_OUTBOUND_WEB_IDENTITY_FEDERATION: &str =
    "iam:DisableOutboundWebIdentityFederation";
