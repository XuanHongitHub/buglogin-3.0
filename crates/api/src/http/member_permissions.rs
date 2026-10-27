use std::collections::BTreeSet;

use buglogin_domain::{member::MemberRole, permissions::Action};

pub(super) const MEMBERSHIP_CAPABILITIES: [&str; 15] = [
    "CAP-WS-001",
    "CAP-TEAM-001",
    "CAP-MEM-001",
    "CAP-RES-001",
    "CAP-AUD-001",
    "CAP-PROF-001",
    "CAP-PROF-002",
    "CAP-PROF-003",
    "CAP-NET-001",
    "CAP-NET-003",
    "CAP-EXT-002",
    "CAP-SYNC-001",
    "events.read",
    "operation.read",
    "profile.read",
];

pub(super) const PROFILE_PERMISSION_ALIASES: [(&str, Action, &str, Action); 10] = [
    ("CAP-PROF-001", Action::Read, "profile.read", Action::Read),
    (
        "CAP-PROF-001",
        Action::Create,
        "profile.create",
        Action::Create,
    ),
    (
        "CAP-PROF-001",
        Action::Update,
        "profile.update",
        Action::Update,
    ),
    (
        "CAP-PROF-001",
        Action::Delete,
        "profile.delete",
        Action::Delete,
    ),
    (
        "CAP-PROF-001",
        Action::Update,
        "profile.archive",
        Action::Update,
    ),
    (
        "CAP-PROF-001",
        Action::Update,
        "profile.restore",
        Action::Update,
    ),
    (
        "CAP-PROF-002",
        Action::Execute,
        "profile.launch",
        Action::Execute,
    ),
    (
        "CAP-PROF-002",
        Action::Execute,
        "profile.stop",
        Action::Execute,
    ),
    (
        "CAP-PROF-001",
        Action::Share,
        "CAP-SHARE-001",
        Action::Manage,
    ),
    ("CAP-PROF-001", Action::Read, "CAP-SHARE-001", Action::Read),
];

const ALL_ACTIONS: [Action; 7] = [
    Action::Read,
    Action::Create,
    Action::Update,
    Action::Delete,
    Action::Execute,
    Action::Share,
    Action::Manage,
];

fn expand_profile_permissions(pairs: &mut BTreeSet<(String, Action)>) {
    for (source, source_action, target, target_action) in PROFILE_PERMISSION_ALIASES {
        if pairs.contains(&(source.to_owned(), source_action)) {
            pairs.insert((target.to_owned(), target_action));
        }
    }
}

pub(super) fn role_default_pairs(role: &MemberRole) -> BTreeSet<(String, Action)> {
    const OPERATE: [Action; 5] = [
        Action::Read,
        Action::Create,
        Action::Update,
        Action::Execute,
        Action::Share,
    ];
    const USE: [Action; 2] = [Action::Read, Action::Execute];
    let mut defaults = BTreeSet::new();
    for capability in MEMBERSHIP_CAPABILITIES {
        let actions: &[Action] = match role {
            MemberRole::Owner | MemberRole::Admin => &ALL_ACTIONS,
            MemberRole::Lead => match capability {
                "CAP-WS-001" | "CAP-AUD-001" | "events.read" | "operation.read"
                | "profile.read" => &[Action::Read],
                "CAP-MEM-001" => &[Action::Read, Action::Create, Action::Manage],
                "CAP-TEAM-001" => &[Action::Read, Action::Create, Action::Update, Action::Manage],
                _ => &OPERATE,
            },
            MemberRole::Operator => match capability {
                "CAP-WS-001" | "CAP-MEM-001" | "CAP-TEAM-001" | "CAP-AUD-001" | "events.read"
                | "operation.read" | "profile.read" => &[Action::Read],
                _ => &OPERATE,
            },
            MemberRole::Member => match capability {
                "CAP-WS-001" | "CAP-MEM-001" | "CAP-TEAM-001" | "CAP-AUD-001" | "events.read"
                | "operation.read" | "profile.read" => &[Action::Read],
                _ => &USE,
            },
        };
        defaults.extend(
            actions
                .iter()
                .map(|action| (capability.to_owned(), *action)),
        );
    }
    expand_profile_permissions(&mut defaults);
    defaults
}

#[cfg(test)]
mod tests {
    use buglogin_domain::{member::MemberRole, permissions::Action};

    use super::role_default_pairs;

    #[test]
    fn lead_defaults_are_workspace_scoped_and_cannot_delete_or_manage_policy() {
        let defaults = role_default_pairs(&MemberRole::Lead);
        assert!(defaults.contains(&("CAP-MEM-001".into(), Action::Manage)));
        assert!(defaults.contains(&("CAP-MEM-001".into(), Action::Create)));
        assert!(defaults.contains(&("CAP-TEAM-001".into(), Action::Manage)));
        assert!(defaults.contains(&("CAP-PROF-001".into(), Action::Update)));
        assert!(!defaults.contains(&("CAP-MEM-001".into(), Action::Delete)));
        assert!(!defaults.contains(&("CAP-WS-001".into(), Action::Manage)));
        assert!(!defaults.contains(&("CAP-WS-001".into(), Action::Update)));
        assert!(!defaults.contains(&("CAP-ADM-001".into(), Action::Create)));
    }

    #[test]
    fn owner_and_member_profile_access_follow_role_limits() {
        assert!(
            role_default_pairs(&MemberRole::Owner)
                .contains(&("profile.create".into(), Action::Create))
        );
        assert!(
            !role_default_pairs(&MemberRole::Member)
                .contains(&("profile.create".into(), Action::Create))
        );
    }
}
