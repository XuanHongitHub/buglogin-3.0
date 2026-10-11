# MIG-025 Locked Files

The profile Preferences file is opened with exclusive sharing on Windows during inspection. Snapshot must return `migration_source_locked`, create no package or destination state, then succeed unchanged after the lock is released.
