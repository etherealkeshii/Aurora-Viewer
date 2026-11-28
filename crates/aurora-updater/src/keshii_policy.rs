use crate::{feed, signature};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const POLICY_URL: &str = "https://raw.githubusercontent.com/etherealkeshii/Aurora-Viewer/release-policy/keshii-policy.json";

pub const SIGNATURE_URL: &str = "https://raw.githubusercontent.com/etherealkeshii/Aurora-Viewer/release-policy/keshii-policy.json.sig";

pub const DEFAULT_GRACE_HOURS: u64 = 168;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema: u32,
    pub revision: u64,
    pub latest_version: String,
    pub expiry_enabled: bool,
    pub grace_hours: u64,
    pub exempt_versions: Vec<String>,
}

impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported Keshii policy schema");
        ensure!(self.revision > 0, "invalid policy revision");
        ensure!(self.grace_hours <= 87600, "invalid grace period");
        validate_version(&self.latest_version)?;

        for version in &self.exempt_versions {
            validate_version(version)?;
        }

        Ok(())
    }

    pub fn requires_expiry(&self, installed: &str) -> bool {
        self.expiry_enabled
            && validate_version(installed).is_ok()
            && crate::is_newer(&self.latest_version, installed)
            && !self.exempt_versions.iter().any(|version| version == installed)
    }

    pub fn grace(&self) -> Duration {
        Duration::from_secs(self.grace_hours.saturating_mul(3600))
    }
}

fn validate_version(value: &str) -> Result<()> {
    ensure!(value.ends_with("-Keshii"), "not a Keshii release");
    let version = feed::version(value).context("invalid release version")?;
    ensure!(version.build.is_empty(), "development version");
    Ok(())
}

pub fn verify_policy(bytes: &[u8], signed: &[u8]) -> Result<Policy> {
    signature::verify(&signature::release_keys(), bytes, signed)?;
    let policy: Policy = serde_json::from_slice(bytes)?;
    policy.validate()?;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            schema: 1,
            revision: 1,
            latest_version: "0.9.14-beta.20261126.1-Keshii".into(),
            expiry_enabled: true,
            grace_hours: DEFAULT_GRACE_HOURS,
            exempt_versions: Vec::new(),
        }
    }

    #[test]
    fn superseded_versions_expire() {
        let policy = policy();
        assert!(policy.validate().is_ok());
        assert!(policy.requires_expiry("0.9.14-beta.20261125.1-Keshii"));
        assert!(!policy.requires_expiry(&policy.latest_version));
        assert_eq!(policy.grace(), Duration::from_secs(604800));
    }

    #[test]
    fn emergency_exemption_reinstates_version() {
        let mut policy = policy();
        let installed = "0.9.14-beta.20261125.1-Keshii";
        assert!(policy.requires_expiry(installed));
        policy.exempt_versions.push(installed.into());
        assert!(!policy.requires_expiry(installed));
        policy.exempt_versions.clear();
        assert!(policy.requires_expiry(installed));
    }

    #[test]
    fn global_suspension_disables_expiry() {
        let mut policy = policy();
        policy.expiry_enabled = false;
        assert!(!policy.requires_expiry("0.9.14-beta.20261125.1-Keshii"));
    }

    #[test]
    fn invalid_policies_are_rejected() {
        let mut policy = policy();
        policy.grace_hours = u64::MAX;
        assert!(policy.validate().is_err());
        policy.grace_hours = DEFAULT_GRACE_HOURS;
        policy.schema = 2;
        assert!(policy.validate().is_err());
    }

    #[test]
    fn release_sequences_sort_numerically() {
        assert!(crate::is_newer("0.9.14-beta.20261125.10-Keshii", "0.9.14-beta.20261125.2-Keshii"));
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignedPolicy {
    pub bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SignedPolicy {
    pub fn verify(&self) -> Result<Policy> {
        verify_policy(&self.bytes, &self.signature)
    }
}

pub async fn fetch() -> Result<SignedPolicy> {
    let client = aurora_proxy::http_client(|builder| {
        builder
            .https_only(true)
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
    })?;

    let (bytes, signed) = tokio::try_join!(fetch_bytes(&client, POLICY_URL, 65536), fetch_bytes(&client, SIGNATURE_URL, 256))?;

    let policy = SignedPolicy { bytes, signature: signed };

    policy.verify()?;
    Ok(policy)
}

async fn fetch_bytes(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .header("Cache-Control", "no-cache")
        .send()
        .await?
        .error_for_status()?;

    let mut bytes = Vec::new();

    while let Some(chunk) = response.chunk().await? {
        ensure!(bytes.len().saturating_add(chunk.len()) <= limit, "Keshii policy response too large");
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

pub fn expiry_deadline(policy: &Policy, installed: &str, first_seen: u64) -> Option<u64> {
    if !policy.requires_expiry(installed) {
        return None;
    }

    Some(first_seen.saturating_add(policy.grace().as_secs()))
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn newer_releases_do_not_extend_existing_deadline() {
        let mut policy = Policy {
            schema: 1,
            revision: 1,
            latest_version: "0.9.14-beta.20261126.1-Keshii".into(),
            expiry_enabled: true,
            grace_hours: DEFAULT_GRACE_HOURS,
            exempt_versions: Vec::new(),
        };

        let installed = "0.9.14-beta.20261125.1-Keshii";
        let first_seen = 1_000_000;
        let expected = Some(first_seen + 604800);

        assert_eq!(expiry_deadline(&policy, installed, first_seen), expected);

        policy.revision += 1;
        policy.latest_version = "0.9.14-beta.20261127.1-Keshii".into();

        assert_eq!(expiry_deadline(&policy, installed, first_seen), expected);

        policy.exempt_versions.push(installed.into());

        assert_eq!(expiry_deadline(&policy, installed, first_seen), None);

        policy.exempt_versions.clear();

        assert_eq!(expiry_deadline(&policy, installed, first_seen), expected);
    }
}

pub fn validate_revision(policy: &Policy, accepted_revision: u64) -> Result<()> {
    ensure!(policy.revision >= accepted_revision, "outdated Keshii policy revision");
    Ok(())
}

#[cfg(test)]
mod revision_tests {
    use super::*;

    #[test]
    fn older_policy_cannot_override_newer_policy() {
        let mut policy = Policy {
            schema: 1,
            revision: 10,
            latest_version: "0.9.14-beta.20261126.1-Keshii".into(),
            expiry_enabled: true,
            grace_hours: DEFAULT_GRACE_HOURS,
            exempt_versions: Vec::new(),
        };

        assert!(validate_revision(&policy, 9).is_ok());
        assert!(validate_revision(&policy, 10).is_ok());
        assert!(validate_revision(&policy, 11).is_err());

        policy.revision = 12;
        assert!(validate_revision(&policy, 11).is_ok());
    }
}

pub fn validate_transition(incoming: &Policy, accepted: Option<&Policy>) -> Result<()> {
    incoming.validate()?;

    if let Some(previous) = accepted {
        validate_revision(incoming, previous.revision)?;

        ensure!(
            incoming.revision != previous.revision || incoming == previous,
            "conflicting Keshii policy with identical revision"
        );
    }

    Ok(())
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    #[test]
    fn policy_transitions_preserve_administrative_authority() {
        let accepted = Policy {
            schema: 1,
            revision: 10,
            latest_version: "0.9.14-beta.20261126.1-Keshii".into(),
            expiry_enabled: true,
            grace_hours: DEFAULT_GRACE_HOURS,
            exempt_versions: Vec::new(),
        };

        assert!(validate_transition(&accepted, None).is_ok());
        assert!(validate_transition(&accepted, Some(&accepted)).is_ok());

        let mut incoming = accepted.clone();
        incoming.expiry_enabled = false;

        assert!(validate_transition(&incoming, Some(&accepted)).is_err());

        incoming.revision = 9;

        assert!(validate_transition(&incoming, Some(&accepted)).is_err());

        incoming.revision = 11;

        assert!(validate_transition(&incoming, Some(&accepted)).is_ok());
    }
}
