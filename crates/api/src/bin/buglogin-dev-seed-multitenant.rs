use buglogin_storage::control::{
    ControlRecordStore, PostgresConfig, PostgresControlRecordStore, PutControlRecord,
};
use serde_json::{Value, json};
use std::{env, process};

fn main() {
    if let Err(code) = run() {
        eprintln!("BugLogin multi-tenant seeding failed: {code}");
        process::exit(1);
    }
}

fn run() -> Result<(), &'static str> {
    let database_url = env::var("BUGLOGIN_POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://buglogin_dev:rrHjxvIgRxpuHEyIMNutbyW0b3voOhBLEfhrYNuveWM@127.0.0.1:5432/buglogin_dev?sslmode=require".to_string());

    let root_cert = env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let default_cert =
                std::path::PathBuf::from(r"C:\ProgramData\BugLogin\pg-tls\root-ca.crt");
            if default_cert.exists() {
                Some(default_cert)
            } else {
                None
            }
        });

    let config = PostgresConfig::new_local_development(database_url, root_cert)
        .map_err(|_| "local_postgres_config_invalid")?;

    let mut store =
        PostgresControlRecordStore::connect(&config).map_err(|_| "local_postgres_unavailable")?;

    let record = store
        .get("control", "control_state", "primary")
        .map_err(|_| "control_record_get_failed")?
        .ok_or("control_state_primary_not_found")?;

    let mut root: Value = record.value.clone();
    let user_account_id = "acct_69038b73abfdca68f8e816421b09d6ab";
    let dev_owner_id = "acct_cc38cc3b46941925c7706c27f82ec88f";
    let now_str = "2026-10-20T08:00:00.0000000Z";

    // 1. Setup Organizations & Workspaces
    let orgs_obj = root
        .get_mut("workspaces")
        .and_then(|w| w.get_mut("organizations"))
        .and_then(Value::as_object_mut)
        .ok_or("invalid_organizations_structure")?;

    // Org 1: Alpha Media Agency (User = Owner, Cấp 4)
    orgs_obj.insert(
        "org_alpha_enterprise".to_string(),
        json!({
            "organization": {
                "id": "org_alpha_enterprise",
                "name": "Alpha Media Agency",
                "accountId": user_account_id,
                "createdAt": now_str
            },
            "policy": {
                "maxWorkspaces": 10,
                "offlineEntitled": false
            },
            "revision": 1,
            "workspaces": {
                "ws_shopee_global": {
                    "id": "ws_shopee_global",
                    "name": "Shopee Global Operations",
                    "organizationId": "org_alpha_enterprise",
                    "policy": {
                        "maxMembers": 15,
                        "maxProfiles": 50,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                },
                "ws_lazada_sea": {
                    "id": "ws_lazada_sea",
                    "name": "Lazada SEA Mall",
                    "organizationId": "org_alpha_enterprise",
                    "policy": {
                        "maxMembers": 10,
                        "maxProfiles": 30,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                }
            },
            "workspace_revisions": {
                "ws_shopee_global": 1,
                "ws_lazada_sea": 1
            }
        }),
    );

    // Org 2: MMO Top Studio (User = Admin, Cấp 3)
    orgs_obj.insert(
        "org_mmo_studio".to_string(),
        json!({
            "organization": {
                "id": "org_mmo_studio",
                "name": "MMO Top Studio",
                "accountId": dev_owner_id,
                "createdAt": now_str
            },
            "policy": {
                "maxWorkspaces": 8,
                "offlineEntitled": false
            },
            "revision": 1,
            "workspaces": {
                "ws_facebook_farming": {
                    "id": "ws_facebook_farming",
                    "name": "Facebook Farming & Automation",
                    "organizationId": "org_mmo_studio",
                    "policy": {
                        "maxMembers": 20,
                        "maxProfiles": 100,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                },
                "ws_google_ads_bm": {
                    "id": "ws_google_ads_bm",
                    "name": "Google Ads Agency BM",
                    "organizationId": "org_mmo_studio",
                    "policy": {
                        "maxMembers": 10,
                        "maxProfiles": 50,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                }
            },
            "workspace_revisions": {
                "ws_facebook_farming": 1,
                "ws_google_ads_bm": 1
            }
        }),
    );

    // Org 3: Global Ecom Partner (User = Team Lead / Operator, Cấp 2)
    orgs_obj.insert(
        "org_global_ecom".to_string(),
        json!({
            "organization": {
                "id": "org_global_ecom",
                "name": "Global Ecom Partner",
                "accountId": dev_owner_id,
                "createdAt": now_str
            },
            "policy": {
                "maxWorkspaces": 5,
                "offlineEntitled": false
            },
            "revision": 1,
            "workspaces": {
                "ws_tiktok_ads_us": {
                    "id": "ws_tiktok_ads_us",
                    "name": "TikTok Ads US & Dropship",
                    "organizationId": "org_global_ecom",
                    "policy": {
                        "maxMembers": 8,
                        "maxProfiles": 30,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                }
            },
            "workspace_revisions": {
                "ws_tiktok_ads_us": 1
            }
        }),
    );

    // Org 4: Freelance Client Hub (User = Member, Cấp 1)
    orgs_obj.insert(
        "org_freelance_client".to_string(),
        json!({
            "organization": {
                "id": "org_freelance_client",
                "name": "Freelance Client Hub",
                "accountId": dev_owner_id,
                "createdAt": now_str
            },
            "policy": {
                "maxWorkspaces": 5,
                "offlineEntitled": false
            },
            "revision": 1,
            "workspaces": {
                "ws_etsy_clean": {
                    "id": "ws_etsy_clean",
                    "name": "Etsy Clean Store Ops",
                    "organizationId": "org_freelance_client",
                    "policy": {
                        "maxMembers": 5,
                        "maxProfiles": 10,
                        "maxStorageBytes": 1073741824,
                        "offlineEntitled": false
                    },
                    "createdAt": now_str
                }
            },
            "workspace_revisions": {
                "ws_etsy_clean": 1
            }
        }),
    );

    // 2. Setup Memberships
    let members_obj = root
        .get_mut("members")
        .and_then(|m| m.get_mut("members"))
        .and_then(Value::as_object_mut)
        .ok_or("invalid_members_structure")?;

    let memberships = [
        (
            "mem_seed_alpha_shopee",
            user_account_id,
            "ws_shopee_global",
            "owner",
        ),
        (
            "mem_seed_alpha_lazada",
            user_account_id,
            "ws_lazada_sea",
            "owner",
        ),
        (
            "mem_seed_mmo_fb_owner",
            dev_owner_id,
            "ws_facebook_farming",
            "owner",
        ),
        (
            "mem_seed_mmo_fb_admin",
            user_account_id,
            "ws_facebook_farming",
            "admin",
        ),
        (
            "mem_seed_mmo_gg_owner",
            dev_owner_id,
            "ws_google_ads_bm",
            "owner",
        ),
        (
            "mem_seed_mmo_gg_admin",
            user_account_id,
            "ws_google_ads_bm",
            "admin",
        ),
        (
            "mem_seed_ecom_tt_owner",
            dev_owner_id,
            "ws_tiktok_ads_us",
            "owner",
        ),
        (
            "mem_seed_ecom_tt_lead",
            user_account_id,
            "ws_tiktok_ads_us",
            "operator",
        ),
        (
            "mem_seed_freelance_owner",
            dev_owner_id,
            "ws_etsy_clean",
            "owner",
        ),
        (
            "mem_seed_freelance_mem",
            user_account_id,
            "ws_etsy_clean",
            "member",
        ),
    ];

    for (id, acc, ws, role) in memberships {
        members_obj.insert(
            id.to_string(),
            json!({
                "id": id,
                "role": role,
                "status": "active",
                "teamId": null,
                "accountId": acc,
                "workspaceId": ws,
                "inviteExpiresAt": null
            }),
        );
    }

    // Keep existing profiles intact - remove any unlinked test profiles from previous run
    if let Some(prof_obj) = root.get_mut("profiles").and_then(Value::as_object_mut) {
        prof_obj.retain(|k, _| !k.starts_with("prf_seed_"));
    }

    // 3. Save updated control_state to PostgreSQL
    let idempotency_key = format!(
        "seed-multitenant-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let put_req = PutControlRecord {
        tenant_id: "control",
        record_type: "control_state",
        record_id: "primary",
        expected_revision: Some(record.revision),
        idempotency_key: &idempotency_key,
        value: &root,
    };

    store.put(put_req).map_err(|_| "store_put_failed")?;

    println!("Multi-tenant seeding completed successfully!");
    println!("Account: {}", user_account_id);
    println!("- Org 1: Alpha Media Agency -> Role: Owner (Cấp 4)");
    println!("  - ws_shopee_global: Shopee Global Operations");
    println!("  - ws_lazada_sea: Lazada SEA Mall");
    println!("- Org 2: MMO Top Studio -> Role: Admin (Cấp 3)");
    println!("  - ws_facebook_farming: Facebook Farming & Automation");
    println!("  - ws_google_ads_bm: Google Ads Agency BM");
    println!("- Org 3: Global Ecom Partner -> Role: Team Lead / Operator (Cấp 2)");
    println!("  - ws_tiktok_ads_us: TikTok Ads US & Dropship");
    println!("- Org 4: Freelance Client Hub -> Role: Member (Cấp 1)");
    println!("  - ws_etsy_clean: Etsy Clean Store Ops");

    Ok(())
}
