use std::collections::HashSet;

use buglogin_api::{
    commands::{Adapter, CommandRegistry},
    mcp::{self, DispatchError},
};
use buglogin_domain::permissions::Action;

#[test]
fn tools_are_derived_from_mcp_enabled_registry_commands() {
    assert!(!mcp::ENABLED_BY_DEFAULT);
    let expected: HashSet<_> = CommandRegistry::all()
        .iter()
        .filter(|command| command.adapters.contains(&Adapter::Mcp))
        .map(|command| command.id.as_str().replace('.', "_"))
        .collect();
    let actual: HashSet<_> = mcp::tools().into_iter().map(|tool| tool.name).collect();

    assert_eq!(actual, expected);
}

#[test]
fn tool_metadata_actions_match_the_canonical_registry() {
    for tool in mcp::tools() {
        let command = CommandRegistry::get(tool.command_id).expect("registered MCP command");

        assert_eq!(tool.action, command.action, "{}", tool.command_id);
    }
}

#[test]
fn dispatch_resolves_back_to_the_canonical_command() {
    let dispatch = mcp::dispatch_metadata("profile_launch").expect("MCP tool");

    assert_eq!(dispatch.command.id.as_str(), "profile.launch");
    assert_eq!(dispatch.command.capability, "profile.launch");
    assert_eq!(dispatch.command.action, Action::Execute);
    assert!(dispatch.scope_required);
    assert!(!dispatch.secrets_returned);
}

#[test]
fn dispatch_rejects_unknown_and_non_mcp_commands() {
    assert_eq!(
        mcp::dispatch_metadata("missing_tool"),
        Err(DispatchError::UnknownTool)
    );
    assert_eq!(
        mcp::dispatch_metadata("migration_import"),
        Err(DispatchError::AdapterNotEnabled)
    );
}

#[test]
fn mutation_metadata_preserves_registry_security_requirements() {
    for tool in mcp::tools()
        .into_iter()
        .filter(|tool| tool.idempotency_required)
    {
        assert!(tool.auth_required, "{}", tool.command_id);
        assert!(tool.audit_required, "{}", tool.command_id);
    }
}
