use std::collections::HashSet;

use buglogin_api::commands::{Adapter, CommandKind, CommandRegistry};
use buglogin_domain::permissions::Action;
use sha2::{Digest, Sha256};

const REGISTRY_SOURCE: &str = include_str!("../../../docs/contracts/command-registry-v1.yaml");

#[test]
fn registry_contains_every_v1_command_once() {
    let commands = CommandRegistry::all();
    assert!(!commands.is_empty());
    let ids: HashSet<_> = commands.iter().map(|command| command.id.as_str()).collect();
    assert_eq!(ids.len(), commands.len());
}

#[test]
fn generated_registry_matches_canonical_source_hash() {
    let source_hash = format!("{:x}", Sha256::digest(REGISTRY_SOURCE.as_bytes()));
    assert_eq!(CommandRegistry::source_sha256(), source_hash);
}

#[test]
fn every_registered_command_has_canonical_action() {
    let expected = [
        ("system.health.get", Action::Read),
        ("operation.get", Action::Read),
        ("events.stream", Action::Read),
        ("profile.list", Action::Read),
        ("profile.get", Action::Read),
        ("profile.create", Action::Create),
        ("profile.clone", Action::Create),
        ("profile.share", Action::Manage),
        ("profile.share.revoke", Action::Manage),
        ("profile.launch", Action::Execute),
        ("profile.stop", Action::Execute),
        ("profile.sync.recovery.code.get", Action::Read),
        ("profile.sync.recover", Action::Read),
        ("profile.delete", Action::Delete),
        ("profile.archive", Action::Update),
        ("profile.restore", Action::Update),
        ("cookie.list", Action::Read),
        ("cookie.stage", Action::Create),
        ("cookie.import", Action::Create),
        ("cookie.copy", Action::Create),
        ("proxy.parse", Action::Read),
        ("proxy.list", Action::Read),
        ("proxy.get", Action::Read),
        ("proxy.create", Action::Create),
        ("proxy.update", Action::Update),
        ("proxy.delete", Action::Delete),
        ("migration.inspect", Action::Read),
        ("migration.import", Action::Execute),
        ("migration.sync.import", Action::Execute),
        ("migration.package.file.put", Action::Execute),
        ("migration.package.verify", Action::Execute),
        ("migration.package.activate", Action::Execute),
        ("auth.device.start", Action::Create),
        ("auth.login", Action::Create),
        ("auth.api.tokens.list", Action::Read),
        ("auth.api.token.create", Action::Create),
        ("auth.api.token.revoke", Action::Update),
        ("auth.recovery.request", Action::Create),
        ("auth.recovery.complete", Action::Update),
        ("auth.session", Action::Read),
        ("auth.sessions.list", Action::Read),
        ("auth.sessions.revoke", Action::Update),
        ("auth.device.approve", Action::Update),
        ("auth.device.token", Action::Create),
        ("desktop.auth.status", Action::Read),
        ("desktop.auth.device.start", Action::Execute),
        ("desktop.auth.device.poll", Action::Execute),
        ("desktop.auth.logout", Action::Update),
        ("auth.refresh", Action::Update),
        ("auth.logout", Action::Update),
        ("profile.runtime.get", Action::Read),
        ("profile.runtime.started", Action::Update),
        ("profile.runtime.stopped", Action::Update),
        ("resource.lease.acquire", Action::Execute),
        ("resource.lease.renew", Action::Execute),
        ("resource.lease.release", Action::Execute),
        ("profile.manifest.get", Action::Read),
        ("profile.manifest.commit", Action::Update),
        ("profile.blob.get", Action::Read),
        ("profile.blob.put", Action::Update),
        ("account.get", Action::Read),
        ("organization.list", Action::Read),
        ("organization.get", Action::Read),
        ("organization.create", Action::Create),
        ("organization.update", Action::Update),
        ("workspace.list", Action::Read),
        ("workspace.get", Action::Read),
        ("workspace.create", Action::Create),
        ("workspace.update", Action::Update),
        ("team.list", Action::Read),
        ("team.get", Action::Read),
        ("team.create", Action::Create),
        ("team.update", Action::Update),
        ("member.list", Action::Read),
        ("member.get", Action::Read),
        ("member.invite", Action::Create),
        ("member.activate", Action::Update),
        ("member.suspend", Action::Manage),
        ("member.role.update", Action::Manage),
        ("resource.list", Action::Read),
        ("resource.get", Action::Read),
        ("resource.register", Action::Create),
        ("grant.list", Action::Read),
        ("grant.create", Action::Manage),
        ("grant.revoke", Action::Manage),
    ];

    assert_eq!(expected.len(), CommandRegistry::all().len());
    for (id, action) in expected {
        assert_eq!(CommandRegistry::get(id).expect(id).action, action, "{id}");
    }
}

#[test]
fn control_commands_are_schema_bound_and_mcp_discoverable() {
    let expected = [
        ("account.get", "CAP-ID-001", "Empty", "Account"),
        (
            "organization.list",
            "CAP-ORG-001",
            "OrganizationListInput",
            "OrganizationPage",
        ),
        (
            "organization.get",
            "CAP-ORG-001",
            "OrganizationIdInput",
            "Organization",
        ),
        (
            "organization.create",
            "CAP-ORG-001",
            "OrganizationCreateInput",
            "Organization",
        ),
        (
            "organization.update",
            "CAP-ORG-001",
            "OrganizationUpdateInput",
            "Organization",
        ),
        (
            "workspace.list",
            "CAP-WS-001",
            "WorkspaceListInput",
            "WorkspacePage",
        ),
        (
            "workspace.get",
            "CAP-WS-001",
            "WorkspaceIdInput",
            "Workspace",
        ),
        (
            "workspace.create",
            "CAP-WS-001",
            "WorkspaceCreateInput",
            "Workspace",
        ),
        (
            "workspace.update",
            "CAP-WS-001",
            "WorkspaceUpdateInput",
            "Workspace",
        ),
        ("team.list", "CAP-TEAM-001", "TeamListInput", "TeamPage"),
        ("team.get", "CAP-TEAM-001", "TeamIdInput", "Team"),
        ("team.create", "CAP-TEAM-001", "TeamCreateInput", "Team"),
        ("team.update", "CAP-TEAM-001", "TeamUpdateInput", "Team"),
        (
            "member.list",
            "CAP-MEM-001",
            "MemberListInput",
            "MemberPage",
        ),
        ("member.get", "CAP-MEM-001", "MemberIdInput", "Member"),
        (
            "member.invite",
            "CAP-MEM-001",
            "MemberInviteInput",
            "Member",
        ),
        ("member.activate", "CAP-MEM-001", "MemberIdInput", "Member"),
        ("member.suspend", "CAP-MEM-001", "MemberIdInput", "Member"),
        (
            "member.role.update",
            "CAP-MEM-001",
            "MemberRoleUpdateInput",
            "Member",
        ),
        (
            "resource.list",
            "CAP-RES-001",
            "ResourceListInput",
            "ResourcePage",
        ),
        ("resource.get", "CAP-RES-001", "ResourceIdInput", "Resource"),
        (
            "resource.register",
            "CAP-RES-001",
            "ResourceRegisterInput",
            "Resource",
        ),
        ("grant.list", "CAP-SHARE-001", "GrantListInput", "GrantPage"),
        ("grant.create", "CAP-SHARE-001", "GrantCreateInput", "Grant"),
        (
            "grant.revoke",
            "CAP-SHARE-001",
            "GrantIdInput",
            "GrantRevocationReceipt",
        ),
    ];

    for (id, capability, input, output) in expected {
        let command = CommandRegistry::get(id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(command.capability, capability, "{id}");
        assert_eq!(command.input_schema.as_str(), input, "{id}");
        assert_eq!(command.output_schema.as_str(), output, "{id}");
        assert!(command.adapters.contains(&Adapter::Mcp), "{id}");
    }
}

#[test]
fn every_registered_command_has_closed_schemas() {
    for command in CommandRegistry::all() {
        assert!(
            !command.input_schema.as_str().is_empty(),
            "{}",
            command.id.as_str()
        );
        assert!(
            !command.output_schema.as_str().is_empty(),
            "{}",
            command.id.as_str()
        );
    }
}

#[test]
fn every_mutation_is_audited_authenticated_and_idempotent() {
    for command in CommandRegistry::all()
        .iter()
        .filter(|command| matches!(command.kind, CommandKind::Command | CommandKind::Operation))
    {
        assert!(command.audit_required, "{}", command.id.as_str());
        if !matches!(
            command.id.as_str(),
            "auth.device.start"
                | "auth.device.token"
                | "auth.login"
                | "auth.recovery.request"
                | "auth.recovery.complete"
                | "desktop.auth.device.start"
                | "desktop.auth.device.poll"
                | "desktop.auth.logout"
        ) {
            assert!(command.auth_required, "{}", command.id.as_str());
        }
        if !matches!(
            command.id.as_str(),
            "auth.device.start"
                | "auth.device.approve"
                | "auth.device.token"
                | "auth.login"
                | "auth.api.token.create"
                | "auth.recovery.request"
                | "desktop.auth.device.start"
                | "desktop.auth.device.poll"
                | "desktop.auth.logout"
        ) {
            assert!(command.idempotency_required, "{}", command.id.as_str());
        }
    }
}

#[test]
fn adapters_match_contract_boundaries() {
    let migration = CommandRegistry::get("migration.import").expect("migration command");
    assert!(migration.adapters.contains(&Adapter::Tauri));
    assert!(migration.adapters.contains(&Adapter::Rest));
    assert!(!migration.adapters.contains(&Adapter::Mcp));
    assert!(
        CommandRegistry::get("profile.launch")
            .expect("profile launch")
            .adapters
            .contains(&Adapter::Mcp)
    );
}
