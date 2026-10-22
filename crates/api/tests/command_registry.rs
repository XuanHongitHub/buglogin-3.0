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
fn cookie_value_mutations_are_native_adapter_only() {
    for command_id in ["cookie.stage", "cookie.import", "cookie.copy"] {
        let command = CommandRegistry::get(command_id).expect("cookie command is registered");
        assert_eq!(command.adapters, &[Adapter::Tauri], "{command_id}");
    }
}

#[test]
fn every_registered_command_has_canonical_action() {
    let expected = [
        ("runtime.catalog.get", Action::Read),
        ("system.health.get", Action::Read),
        ("operation.get", Action::Read),
        ("events.stream", Action::Read),
        ("audit.events.list", Action::Read),
        ("profile.list", Action::Read),
        ("profile.get", Action::Read),
        ("profile.binding.get", Action::Read),
        ("profile.create", Action::Create),
        ("profile.update", Action::Update),
        ("profile.clone", Action::Create),
        ("profile.share", Action::Manage),
        ("profile.share.revoke", Action::Manage),
        ("profile.share.recipientdevicekeys.list", Action::Manage),
        ("profile.share.keyenvelope.get", Action::Read),
        ("profile.keyrotation.recipients.list", Action::Update),
        ("profile.keyrotation.execute", Action::Update),
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
        ("cookie.jobs.inbox", Action::Read),
        ("proxy.parse", Action::Read),
        ("proxy.import", Action::Create),
        ("proxy.list", Action::Read),
        ("proxy.get", Action::Read),
        ("proxy.create", Action::Create),
        ("proxy.update", Action::Update),
        ("proxy.delete", Action::Delete),
        ("proxy.check", Action::Read),
        ("migration.inspect", Action::Read),
        ("migration.import", Action::Execute),
        ("migration.profile.data.import", Action::Update),
        ("migration.sync.import", Action::Execute),
        ("migration.control.import", Action::Execute),
        ("migration.control.rollback", Action::Execute),
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
        ("auth.device.refresh", Action::Update),
        ("auth.desktop.login", Action::Create),
        ("auth.device.encryptionkey.register", Action::Create),
        ("desktop.auth.status", Action::Read),
        ("desktop.auth.login", Action::Create),
        ("desktop.auth.logout", Action::Update),
        ("auth.refresh", Action::Update),
        ("auth.logout", Action::Update),
        ("profile.runtime.get", Action::Read),
        ("profile.runtime.started", Action::Update),
        ("profile.runtime.stopped", Action::Update),
        ("resource.lease.acquire", Action::Execute),
        ("resource.lease.renew", Action::Execute),
        ("resource.lease.release", Action::Execute),
        ("resource.lease.get", Action::Read),
        ("profile.manifest.get", Action::Read),
        ("profile.manifest.commit", Action::Update),
        ("profile.blob.get", Action::Read),
        ("profile.blob.check", Action::Update),
        ("profile.blob.put", Action::Update),
        ("account.get", Action::Read),
        ("account.list", Action::Read),
        ("account.status.update", Action::Manage),
        ("organization.list", Action::Read),
        ("organization.get", Action::Read),
        ("organization.record.get", Action::Read),
        ("organization.create", Action::Create),
        ("organization.update", Action::Update),
        ("workspace.list", Action::Read),
        ("workspace.get", Action::Read),
        ("workspace.record.get", Action::Read),
        ("workspace.create", Action::Create),
        ("workspace.update", Action::Update),
        ("group.list", Action::Read),
        ("group.get", Action::Read),
        ("group.create", Action::Create),
        ("group.update", Action::Update),
        ("group.delete", Action::Delete),
        ("extension.group.list", Action::Read),
        ("extension.group.get", Action::Read),
        ("extension.group.create", Action::Create),
        ("extension.group.update", Action::Update),
        ("extension.group.delete", Action::Delete),
        ("profile.extension.group.set", Action::Update),
        ("group.share.list", Action::Read),
        ("group.share.create", Action::Share),
        ("group.share.revoke", Action::Share),
        ("profile.groups.replace", Action::Update),
        ("profile.proxy.assignment.get", Action::Read),
        ("profile.proxy.assignment.set", Action::Update),
        ("team.list", Action::Read),
        ("team.get", Action::Read),
        ("team.create", Action::Create),
        ("team.update", Action::Update),
        ("team.delete", Action::Delete),
        ("member.list", Action::Read),
        ("member.get", Action::Read),
        ("member.invite", Action::Create),
        ("member.invitation.create", Action::Create),
        ("member.invitation.redeem", Action::Update),
        ("member.invitation.revoke", Action::Manage),
        ("member.activate", Action::Update),
        ("member.suspend", Action::Manage),
        ("member.role.update", Action::Manage),
        ("resource.list", Action::Read),
        ("resource.get", Action::Read),
        ("resource.register", Action::Create),
        ("grant.list", Action::Read),
        ("grant.list.legacy", Action::Read),
        ("grant.list.page", Action::Read),
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
        ("account.get", "CAP-ID-001", "Empty", "AccountView"),
        (
            "organization.list",
            "CAP-ORG-001",
            "Empty",
            "OrganizationPage",
        ),
        (
            "organization.get",
            "CAP-ORG-001",
            "OrganizationIdInput",
            "Organization",
        ),
        (
            "organization.record.get",
            "CAP-ORG-001",
            "OrganizationIdInput",
            "OrganizationRecord",
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
            "workspace.record.get",
            "CAP-WS-001",
            "WorkspaceIdInput",
            "WorkspaceRecord",
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
        (
            "team.list",
            "CAP-TEAM-001",
            "TeamListInput",
            "TeamRecordList",
        ),
        ("team.get", "CAP-TEAM-001", "TeamIdInput", "TeamRecord"),
        (
            "team.create",
            "CAP-TEAM-001",
            "TeamCreateInput",
            "TeamRecord",
        ),
        (
            "team.update",
            "CAP-TEAM-001",
            "TeamUpdateInput",
            "TeamRecord",
        ),
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
            "resource.lease.get",
            "profile.read",
            "ResourceIdInput",
            "OptionalLeaseStatus",
        ),
        (
            "resource.register",
            "CAP-RES-001",
            "ResourceRegisterInput",
            "Resource",
        ),
        ("grant.list", "CAP-SHARE-001", "GrantListInput", "GrantPage"),
        (
            "grant.list.page",
            "CAP-SHARE-001",
            "GrantListPageInput",
            "GrantPage",
        ),
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

    let delete = CommandRegistry::get("team.delete").expect("team delete");
    assert_eq!(delete.capability, "CAP-TEAM-001");
    assert_eq!(delete.input_schema.as_str(), "TeamDeleteInput");
    assert_eq!(delete.output_schema.as_str(), "TeamRecord");
    assert_eq!(delete.adapters, &[Adapter::Rest]);

    let profile_data_import =
        CommandRegistry::get("migration.profile.data.import").expect("profile data import");
    assert_eq!(profile_data_import.capability, "profile.update");
    assert_eq!(profile_data_import.action, Action::Update);
    assert_eq!(profile_data_import.adapters, &[Adapter::Tauri]);

    let legacy_grant_list = CommandRegistry::get("grant.list.legacy").expect("legacy grant list");
    assert_eq!(legacy_grant_list.input_schema.as_str(), "GrantListInput");
    assert_eq!(legacy_grant_list.output_schema.as_str(), "GrantList");
    assert_eq!(legacy_grant_list.adapters, &[Adapter::Rest]);
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
                | "auth.device.refresh"
                | "auth.login"
                | "auth.recovery.request"
                | "auth.recovery.complete"
                | "auth.desktop.login"
                | "desktop.auth.login"
                | "desktop.auth.logout"
        ) {
            assert!(command.auth_required, "{}", command.id.as_str());
        }
        if !matches!(
            command.id.as_str(),
            "auth.device.start"
                | "auth.device.approve"
                | "auth.device.token"
                | "auth.device.refresh"
                | "auth.refresh"
                | "auth.login"
                | "auth.api.token.create"
                | "auth.recovery.request"
                | "auth.desktop.login"
                | "desktop.auth.login"
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
