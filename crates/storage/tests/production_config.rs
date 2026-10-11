#![cfg(feature = "integration-tests")]

use buglogin_storage::control::PostgresConfig;
use buglogin_storage::remote::S3Config;

#[test]
fn production_configs_require_tls_without_contacting_services() {
    assert!(PostgresConfig::new("postgres://db/app?sslmode=disable").is_err());
    assert!(PostgresConfig::new("postgres://db/app?sslmode=require").is_ok());
    assert!(S3Config::new("http://objects", "b", "r", "t", "p", "a", "s", None, true).is_err());
    assert!(S3Config::new("https://objects", "b", "r", "t", "p", "a", "s", None, true).is_ok());
}

#[test]
fn s3_namespace_is_tenant_scoped() {
    let config = S3Config::new(
        "https://objects",
        "chunks",
        "us-east-1",
        "tenant-7",
        "buglogin/v1",
        "access",
        "secret",
        None,
        true,
    )
    .unwrap();
    assert_eq!(
        config.namespaced_key("workspace/profile/chunk").unwrap(),
        "buglogin/v1/tenants/tenant-7/workspace/profile/chunk"
    );
}
