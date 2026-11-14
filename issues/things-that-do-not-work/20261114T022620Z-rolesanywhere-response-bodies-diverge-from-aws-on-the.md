Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-listing-and-subjects
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-listing-and-subjects rolesanywhere-not-found-error-paths rolesanywhere-profile-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|GetSubject|4xx->4xx|$.message
Issue-Key: rolesanywhere|EnableProfile|2xx->2xx|$.profile.attributeMappings[*]
Issue-Key: rolesanywhere|EnableProfile|2xx->2xx|$.profile.createdBy
Issue-Key: rolesanywhere|EnableProfile|2xx->2xx|$.profile.managedPolicyArns
Issue-Key: rolesanywhere|EnableProfile|2xx->2xx|$.profile.requireInstanceProperties
Issue-Commit: 7178e01e2fdf9b066fd2aad4180957cad1a114a8

# rolesanywhere response bodies diverge from AWS on the GetSubject not-found refusal and the EnableProfile profile echo

> _Note: this issue was identified against the code base at commit `7178e01e2fdf9b066fd2aad4180957cad1a114a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSubject

## Description
Two net-new root causes on rolesanywhere, both in what a response body carries: a not-found refusal that volunteers a message AWS does not send, and a success echo that reports a profile with the wrong defaults and the wrong creator identity. Two causes, numbered below; a fixer must close BOTH before submitting.

Cause 1 of 2 (GetSubject - the 404 carries a message member AWS does not send):
AWS: Smithy binds GetSubject to GET /subject/{subjectId} (input ScalarSubjectRequest, required @httpLabel subjectId -> Uuid @length 36..36 @pattern [a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}; output SubjectDetailResponse with subject -> SubjectDetail; errors AccessDeniedException httpError 403, ResourceNotFoundException httpError 404, whose only member is an OPTIONAL message -> String). For a well-formed subjectId that no recorded subject carries, AWS answers 404 ResourceNotFoundException with NO message member in the body at all (recorded behavior only - the model documents the error class, not the text; the recorded response's $.message is absent and the error class travels in the x-amzn-errortype header). Absence of the member IS the requirement for this error class on this operation, so there is no message template to render here.
Emulator: the same 404 and the same error class, but it volunteers a message - "The item doesn't exist. arn: <subject ARN> ; aborting." - whose fixed text is "The item doesn't exist. arn: " ... " ; aborting." and whose single variable part is a subject ARN composed from the request's Region/account and the requested subjectId (the concrete ARN in the recorded run is a per-run value filling that template, never part of the requirement). The defect is that ANY message member is serialized on this path.
Why: a client that branches on whether a not-found refusal carries explanatory text, or that compares the refusal body with AWS's, sees a field AWS never sends; worse, the emulator discloses a composed ARN for a subject that does not exist, which AWS deliberately does not echo on this path.
Fix: the GetSubject handler's not-found error construction - omit the message member for this operation's ResourceNotFoundException instead of routing it through the shared ARN-keyed not-found message helper. Related open issue 20261106T145115Z-rolesanywhere-refusal-messages-and-delete-echoes-diverge-on.md carries the same fault for DeleteAttributeMapping only; it does not name GetSubject and carries no replay key for these steps, so it does not cover this cause.
Traces: rolesanywhere-listing-and-subjects#task#5, rolesanywhere-not-found-error-paths#task#15.

Cause 2 of 2 (EnableProfile - the enable echo omits AWS's profile defaults and reports the creator as a bare account id):
AWS: Smithy binds EnableProfile to POST /profile/{profileId}/enable (input ScalarProfileRequest, required @httpLabel profileId -> Uuid @length 36..36 with the UUID pattern; success 200, output ProfileDetailResponse with profile -> ProfileDetail; errors AccessDeniedException 403, ResourceNotFoundException 404). The echoed ProfileDetail is "the state of the profile after a read or write operation", i.e. the same projection every profile read and write returns. For a profile created without attributeMappings and without managedPolicyArns/requireInstanceProperties, AWS's projection (recorded behavior only - the model documents the members, not their defaults) reports: attributeMappings holding the service's three default certificate-field mappings (x509Issuer with mappingRules [{specifier: "*"}], x509SAN with [{specifier: "DNS"}, {specifier: "URI"}, {specifier: "Name/*"}], x509Subject with [{specifier: "*"}]); createdBy as the IAM principal ARN of the caller that created the profile (arn:aws:iam::<account-id>:<principal-type>/<principal-name> - the account id and principal come from the calling identity, only the ARN FORM is the requirement); and NO managedPolicyArns and NO requireInstanceProperties members when the create did not set them.
Emulator: 200 with the right shape, but profile.attributeMappings is empty (the three default mappings are absent), profile.createdBy is the bare account id instead of a principal ARN, and profile.managedPolicyArns ([]) and profile.requireInstanceProperties (false) are serialized unconditionally where AWS omits them. The projection is shared: crate::state::ProfileRecord::to_detail always wraps every member in Some(..), and CreateProfile stores created_by = engine::emulator_account_id() and an empty attribute_mappings vector, so EnableProfile's echo inherits all four divergences.
Why: a client reading the enable response learns the profile's effective certificate-attribute mapping and its owner from it. With no attributeMappings it concludes no certificate fields are mapped (the opposite of AWS, where the defaults map issuer/SAN/subject), and a bare account id where an ARN is expected breaks any consumer that parses createdBy as an ARN or compares it with an IAM principal. The spurious empty managedPolicyArns and false requireInstanceProperties make a client think the profile explicitly set them.
Fix: the shared profile projection and the profile create defaults in service state (ProfileRecord::to_detail and the record CreateProfile builds) - seed the default attribute mappings, record the creating principal's ARN, and omit managedPolicyArns/requireInstanceProperties when nothing set them; EnableProfile's handler then echoes a faithful ProfileDetail with no change of its own. Open issues 20261029T192145Z-rolesanywhere-profile-responses-drop-default-attribute-mappings-and.md and 20261106T184037Z-rolesanywhere-disableprofile-and-deleteprofile-echoes-omit-profile-defaults.md carry this fault for CreateProfile/GetProfile/ListProfiles/UpdateProfile/DisableProfile/DeleteProfile; neither names EnableProfile and neither carries a replay key for this step, so this cause is net-new. The EnableProfile stub issues (20261103T020145Z..., 20261107T014352Z-rolesanywhere-enableprofile-stub.md) describe a 501 scaffold; the handler now answers 200 and persists the flag, so they do not cover this echo divergence either.
Traces: rolesanywhere-profile-lifecycle#task#8.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, /__admin/health 200; 85 steps, 57 passed, score 67.1%). Both clusters below carry an empty stub_path and an empty owned_by.

Cause 1 - GetSubject, expected 404 / actual 404, fixture rolesanywhere-listing-and-subjects step task#5 (cluster also covers rolesanywhere-not-found-error-paths#task#15):
  $.message: type_mismatch (expected None, got "The item doesn't exist. arn: arn:aws:rolesanywhere:us-west-2:000000000000:subject/4a84382e-363e-4b06-9067-216a0c1c0638 ; aborting.")
  AWS sends no $.message for this error class on this operation. Fixed text of the emulator's string: "The item doesn't exist. arn: " + " ; aborting."; variable part: the subject ARN built from the request's Region/account and the requested subjectId (a per-run value, not a requirement).
  Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/get_subject.rs lines 147-156 (message: Some(crate::state::arn_not_found_message(&crate::state::subject_arn(...)))).

Cause 2 - EnableProfile, expected 200 / actual 200, fixture rolesanywhere-profile-lifecycle step task#8:
  $.profile.attributeMappings[0]: missing (expected {'certificateField': 'x509Issuer', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.attributeMappings[1]: missing (expected {'certificateField': 'x509SAN', 'mappingRules': [{'specifier': 'DNS'}, {'specifier': 'URI'}, {'specifier': 'Name/*'}]}, got None)
  $.profile.attributeMappings[2]: missing (expected {'certificateField': 'x509Subject', 'mappingRules': [{'specifier': '*'}]}, got None)
  $.profile.createdBy: value_mismatch (expected an IAM principal ARN of the form 'arn:aws:iam::<account-id>:user/<name>' - recorded as 'arn:aws:iam::000000000000:user/emulator' - got '000000000000')
  $.profile.managedPolicyArns: type_mismatch (expected None, got [])
  $.profile.requireInstanceProperties: type_mismatch (expected None, got False)
  Source: services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/state.rs lines 223-243 (ProfileRecord::to_detail wraps every member in Some(..)); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/create_profile.rs lines 195-219 (created_by = ::engine::emulator_account_id(), attribute_mappings left empty); services/service-com-amazonaws-rolesanywhere-rolesanywhere/src/ops/enable_profile.rs lines 161-166 (writes enabled and echoes record.to_detail()).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay wire trace set simparity-wire-aws-v1.
2. Cause 1: GET /subject/<a well-formed UUID no recorded subject carries> (fixture rolesanywhere-listing-and-subjects, step task#5; also rolesanywhere-not-found-error-paths step task#15). Expect 404 ResourceNotFoundException with NO $.message member in the body (error class only in x-amzn-errortype); observe a $.message of the form "The item doesn't exist. arn: <subject arn> ; aborting.".
3. Cause 2: POST /profiles to create a profile without attributeMappings, managedPolicyArns or requireInstanceProperties, POST /profile/{profileId}/disable, then POST /profile/{profileId}/enable (fixture rolesanywhere-profile-lifecycle, step task#8). Expect the 200 echo's $.profile.attributeMappings to hold the three default mappings (x509Issuer ['*'], x509SAN ['DNS','URI','Name/*'], x509Subject ['*']), $.profile.createdBy to be an IAM principal ARN, and $.profile.managedPolicyArns / $.profile.requireInstanceProperties to be absent; observe empty attributeMappings, a bare account id in createdBy, and both of those members serialized as [] and false.
