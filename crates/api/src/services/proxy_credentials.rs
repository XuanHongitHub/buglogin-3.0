use buglogin_domain::{
    errors::Problem,
    ids::{CredentialId, ProxyId},
};
use sha2::{Digest, Sha256};

/// Build the opaque identifier for a proxy credential generation.
///
/// This value is not secret material. It changes whenever the canonical proxy
/// revision accepts a credential replacement and remains stable for metadata
/// edits that preserve the current credential reference.
pub(crate) fn credential_version(
    proxy_id: &ProxyId,
    revision: u64,
) -> Result<CredentialId, Problem> {
    let mut digest = Sha256::new();
    digest.update(b"buglogin/proxy-credential-version/v1/");
    digest.update(proxy_id.as_str().as_bytes());
    digest.update(revision.to_be_bytes());
    let version = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    CredentialId::new(format!("pcv-{version}")).map_err(|_| {
        Problem::new(
            "invalid_proxy_credential_id",
            "Proxy credential identifier is invalid",
            400,
            "proxy",
        )
    })
}

pub(crate) fn next_proxy_revision(expected_revision: u64) -> Result<u64, Problem> {
    expected_revision.checked_add(1).ok_or_else(|| {
        Problem::new(
            "proxy_revision_exhausted",
            "Proxy revision cannot be incremented",
            409,
            "proxy",
        )
    })
}
