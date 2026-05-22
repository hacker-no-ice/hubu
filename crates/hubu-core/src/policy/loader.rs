use std::{fs, path::Path};

use crate::policy::{engine::validate_policy, error::PolicyLoadError, model::Policy};

impl Policy {
    /// Read, deserialize, and validate a policy from a YAML file.
    ///
    /// This is the recommended entrypoint for human-authored policy files. A
    /// policy that parses but fails validation is returned as
    /// [`PolicyLoadError::Validation`], so invalid field/value combinations are
    /// rejected before evaluation.
    pub fn from_yaml_file(path: impl AsRef<Path>) -> Result<Self, PolicyLoadError> {
        let path = path.as_ref();
        let yaml = fs::read_to_string(path).map_err(|source| PolicyLoadError::ReadFile {
            path: path.display().to_string(),
            source,
        })?;

        Self::from_yaml_str(&yaml)
    }

    /// Deserialize and validate a policy from a YAML string.
    ///
    /// Useful for tests, embedded defaults, and API payloads that have already
    /// been read into memory.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, PolicyLoadError> {
        let policy: Policy = serde_yaml::from_str(yaml)?;
        validate_policy(&policy)?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::policy::{
        error::{PolicyLoadError, PolicyValidationError},
        model::{Effect, Policy},
    };

    fn valid_policy_yaml() -> &'static str {
        r#"
id: base_spending_policy
version: 2026-05-22.1
default_effect: needs_approval
rules:
  - id: deny_large_purchase
    effect: deny
    reason: Purchase exceeds the $500 single-transaction limit
    when:
      op: gt
      field: amount
      value:
        money_cents: 50000

  - id: allow_small_meals
    effect: allow
    reason: Meals under $50 are pre-approved
    when:
      op: all
      conditions:
        - op: eq
          field: category
          value:
            string: meals
        - op: lte
          field: amount
          value:
            money_cents: 5000
"#
    }

    fn write_temp_policy_file(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hubu-policy-{}.yaml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));

        fs::write(&path, contents).expect("test should write temp policy file");
        path
    }

    #[test]
    fn loads_and_validates_policy_from_yaml_str() {
        let policy = Policy::from_yaml_str(valid_policy_yaml()).expect("policy should load");

        assert_eq!(policy.id, "base_spending_policy");
        assert_eq!(policy.version, "2026-05-22.1");
        assert_eq!(policy.default_effect, Effect::NeedsApproval);
        assert_eq!(policy.rules.len(), 2);
    }

    #[test]
    fn loads_and_validates_policy_from_yaml_file() {
        let path = write_temp_policy_file(valid_policy_yaml());

        let policy = Policy::from_yaml_file(&path).expect("policy should load from file");

        assert_eq!(policy.id, "base_spending_policy");
        assert_eq!(policy.rules.len(), 2);

        fs::remove_file(path).expect("test should remove temp policy file");
    }

    #[test]
    fn rejects_yaml_policy_that_fails_validation() {
        let invalid_yaml = r#"
id: base_spending_policy
version: 2026-05-22.1
default_effect: needs_approval
rules:
  - id: invalid_amount_type
    effect: deny
    reason: Amount should use money cents
    when:
      op: gt
      field: amount
      value:
        string: "50000"
"#;

        let error = Policy::from_yaml_str(invalid_yaml).expect_err("policy should be invalid");

        assert!(matches!(
            error,
            PolicyLoadError::Validation {
                source: PolicyValidationError::FieldValueMismatch { .. }
            }
        ));
    }

    #[test]
    fn reports_yaml_parse_errors() {
        let error =
            Policy::from_yaml_str("this is not: [valid yaml").expect_err("yaml should fail");

        assert!(matches!(error, PolicyLoadError::ParseYaml { .. }));
    }
}
