Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-profile-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-profile-lifecycle rolesanywhere-validation-error-paths
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|UpdateProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|UpdateProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|UpdateProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|UpdateProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Key: rolesanywhere|UpdateProfile|4xx->2xx|$.message
Issue-Key: rolesanywhere|UpdateProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rolesanywhere|UpdateProfile|4xx->2xx|status
Issue-Commit: 8be6c2401be6803b94081d6cd0d76b281ab9e586

# rolesanywhere UpdateProfile diverges on its response echo and role-ARN uniqueness validation

> _Note: this issue was identified against the code base at commit `8be6c2401be6803b94081d6cd0d76b281ab9e586`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProfile

## Description
UpdateProfile (PATCH /profile/{profileId}) diverges from AWS in two independent ways: the ProfileDetail it echoes on success is incomplete, and it performs no role-ARN uniqueness validation, so a request AWS refuses with 400 ValidationException is accepted with 200. Two causes, one per failing replay cluster.

Cause 1 of 2 (UpdateProfile success echo, fixture rolesanywhere-profile-lifecycle step task#5):
AWS: UpdateProfile answers 200 with ProfileDetailResponse.profile -> ProfileDetail (Smithy: UpdateProfile output ProfileDetailResponse, http PATCH /profile/{profileId} code 200). That is the SAME ProfileDetail shape every other profile-returning operation serves, so the echo of an updated profile carries: the three service-supplied attributeMappings seeded at create time - certificateField x509Issuer with mappingRules [{specifier '*'}], x509SAN with [{specifier 'DNS'},{specifier 'URI'},{specifier 'Name/*'}], and x509Subject with [{specifier '*'}] (recorded behavior only; the model documents attributeMappings as 'A mapping applied to the authenticating end-entity certificate' but not the defaults); createdBy as the ARN of the principal that created the profile, of the form arn:aws:iam::<account-id>:user/<user-name>, where the arn:aws:iam:: prefix and the :user/ principal path are fixed text and the account id and user name come from the caller (recorded behavior only; the model's doc says only 'The Amazon Web Services account that created the profile'); and NO members for optional values the request never set and the service does not default - managedPolicyArns (Smithy: ProfileDetail.managedPolicyArns, ManagedPolicyList @length 0..50) and requireInstanceProperties (Smithy: ProfileDetail.requireInstanceProperties, Boolean) are OMITTED rather than emitted as [] and false.
Emulator: the 200 echo omits attributeMappings entirely, reports createdBy as the bare 12-digit account id instead of a principal ARN, and emits managedPolicyArns as [] and requireInstanceProperties as false where AWS emits neither member.
Why: a client that updates a profile and uses the returned ProfileDetail as the authoritative post-write state - rather than issuing a second GetProfile - sees no certificate attribute mappings at all, so any logic that reads attributeMappings to learn how x509 fields map to session attributes concludes the profile maps nothing. A client that parses createdBy as an IAM principal ARN fails on a bare account id, and a client that distinguishes 'member absent' from 'empty list / false' records a profile as explicitly having no managed policies and explicitly not requiring instance properties when AWS left both unspecified.
Fix: the UpdateProfile handler's response serialization - it must go through the shared ProfileDetail serializer (default attribute mappings, created-by principal ARN, omission of unset optional members) rather than building its own echo. Related open issue 20261029T192145Z-rolesanywhere-profile-responses-drop-default-attribute-mappings-and.md describes the same ProfileDetail defects on CreateProfile/GetProfile/ListProfiles/DeleteProfile, but its replay keys do not include UpdateProfile's own echo, so this cluster is not covered by it.
Traces: rolesanywhere-profile-lifecycle#task#5.

Cause 2 of 2 (UpdateProfile role-ARN uniqueness, fixture rolesanywhere-validation-error-paths step task#8):
AWS: UpdateProfile models ValidationException (Smithy: UpdateProfile errors AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400; ValidationException members {message: String}). Beyond the member constraints on roleArns (Smithy: UpdateProfile.roleArns -> RoleArnList @length 0..250), the service enforces that the role ARNs a profile references are not already referenced by another profile, and refuses an update that would duplicate them with HTTP 400, header x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/', and body message 'All provided role ARNs should be unique across all Profiles.' (recorded behavior only; the model declares the error but not the rule). That message is entirely fixed text - it names no request value, no id and no limit, so it is reproduced verbatim.
Emulator: the request is accepted. It answers HTTP 200 with a profile body: no $.message member, no x-amzn-errortype header, and no refusal of the duplicate role ARNs at all - the validation is simply absent from the handler.
Why: a client relying on AWS's behavior treats a successful UpdateProfile as proof that the role ARNs it submitted are exclusively owned by that profile. Against the emulator two profiles silently end up referencing the same IAM role, so a caller that tests the duplicate-ARN guard (or writes an error path for it) sees it pass, and the session-vending semantics the uniqueness rule protects are not reproduced. Any negative test for this ValidationException fails to fail.
Fix: UpdateProfile's input validation, which must check the submitted roleArns against the role ARNs held by every other profile in the service state before writing, and raise ValidationException (400) with the fixed message above when any ARN is already claimed.
Traces: rolesanywhere-validation-error-paths#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; 2 mismatch clusters, neither owned by an open issue's replay key and neither blocked by a stub.

Cause 1 - rolesanywhere-profile-lifecycle#task#5, UpdateProfile, expected 200 / actual 200 (body diverged):
  $.profile.attributeMappings[0]: missing (expected {'certificateField': 'x509Issuer', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.attributeMappings[1]: missing (expected {'certificateField': 'x509SAN', 'mappingRules': [{'specifier': 'DNS'}, {'specifier': 'URI'}, {'specifier': 'Name/*'}]}, got None)
  $.profile.attributeMappings[2]: missing (expected {'certificateField': 'x509Subject', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.createdBy: value_mismatch (expected a principal ARN of the form arn:aws:iam::<account-id>:user/<user-name>, got the bare account id)
  $.profile.managedPolicyArns: type_mismatch (expected None - member absent, got [])
  $.profile.requireInstanceProperties: type_mismatch (expected None - member absent, got false)

Cause 2 - rolesanywhere-validation-error-paths#task#8, UpdateProfile, expected 400 / actual 200:
  status: expected 400, got 200
  $.message: missing (expected 'All provided role ARNs should be unique across all Profiles.', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/', got None)

Model references used: Smithy UpdateProfile (input UpdateProfileRequest, output ProfileDetailResponse, errors AccessDeniedException/ResourceNotFoundException/ValidationException, http PATCH /profile/{profileId} code 200), ProfileDetail (attributeMappings, createdBy, managedPolicyArns, requireInstanceProperties), ValidationException (httpError 400).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: fixture rolesanywhere-profile-lifecycle - create a profile (setup#2), then PATCH /profile/{profileId} at step task#5. Inspect the 200 response body: $.profile has no attributeMappings member, createdBy is a bare 12-digit account id instead of an arn:aws:iam::<account-id>:user/<user-name> principal ARN, and managedPolicyArns ([]) / requireInstanceProperties (false) are present although the request never set them and AWS omits both.
3. Cause 2: fixture rolesanywhere-validation-error-paths step task#8 - PATCH /profile/{profileId} with roleArns that are already referenced by another existing profile. The emulator answers 200 with a profile body; AWS answers 400 with x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.rolesanywhere.service/' and message 'All provided role ARNs should be unique across all Profiles.'
4. Both steps must pass before this issue is closed: re-replay rolesanywhere-profile-lifecycle#task#5 AND rolesanywhere-validation-error-paths#task#8.
