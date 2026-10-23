#![forbid(unsafe_code)]

pub mod adapter;
mod agent_features;
mod auth_vault;
mod clock;
mod cookie_tools;
mod desktop_state;
mod device_identity;
mod device_key_vault;
mod event_stream;
mod extension_features;
mod legacy_compat;
mod local_preferences;
mod migration_commands;
mod network_features;
mod profile_cookie_manager;
mod profile_lifecycle;
mod profile_projection;
pub mod profile_runtime;
mod proxy_bridge;
mod proxy_credential_vault;
mod proxy_store;
mod remote_api;
mod runtime_config;
mod runtime_install;
mod sync_features;
mod sync_key_vault;
mod system_features;
mod wayfern_fingerprint;
mod wayfern_terms;

use adapter::AdapterState;
use buglogin_api::commands::CommandResult;
use buglogin_domain::errors::Problem;
use desktop_state::{DesktopAuthStatus, DesktopState};
use serde_json::{Value, json};
use tauri::{Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

use legacy_compat::{
    assign_profiles_to_group, change_profile_password, check_browser_status, check_proxy_validity,
    check_stored_proxy, create_browser_profile_new, create_profile_group, create_stored_proxy,
    delete_profile, delete_profile_group, delete_selected_profiles, delete_stored_proxy,
    get_all_traffic_snapshots, get_available_workspaces, get_browser_profiles, get_groups,
    get_groups_with_profile_counts, get_profile_groups, get_profile_traffic_snapshot,
    get_stored_proxies, import_proxies_from_parsed, import_proxies_json, kill_browser_profile,
    launch_browser_profile, list_browser_profiles, list_running_browser_profiles,
    parse_txt_proxies, remove_profile_password, rename_profile, select_active_workspace,
    set_profile_password, set_profile_sync_mode, update_profile_clear_on_close,
    update_profile_dns_blocklist, update_profile_dns_server, update_profile_group,
    update_profile_launch_hook, update_profile_note, update_profile_proxy,
    update_profile_proxy_bypass_rules, update_profile_start_urls, update_profile_tags,
    update_profile_window_color, update_stored_proxy, update_wayfern_config,
    verify_profile_password,
};
use local_preferences::{
    complete_onboarding, dismiss_window_resize_warning, get_app_settings, get_onboarding_completed,
    get_system_language, get_table_preferences, get_table_sorting_settings,
    get_window_resize_warning_dismissed, save_app_settings, save_table_preferences,
    save_table_sorting_settings,
};
use profile_cookie_manager::{
    export_profile_cookies, export_profile_storage_state, get_profile_cookie_stats,
    get_profile_local_storage_stats, import_profile_cookies, import_profile_storage_state,
};

pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init());
    let updater_configured = option_env!("BUGLOGIN_TAURI_UPDATER_PUBLIC_KEY")
        .is_some_and(|public_key| !public_key.trim().is_empty());
    if updater_configured {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .setup(move |app| {
            let state = build_state().unwrap_or_else(DesktopState::unavailable);
            state.start_background_auth_refresh();
            app.manage(state);
            app.manage(event_stream::EventStreamManager::default());
            if updater_configured {
                let update_app = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(3 * 60 * 60));
                    // The UI checks once on startup. Skip Tokio's immediate first
                    // tick so this task only performs periodic checks afterwards.
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        match system_features::check_for_app_updates(update_app.clone()).await {
                            Ok(Some(update)) => {
                                if let Err(error) = update_app.emit("app-update-available", update)
                                {
                                    eprintln!("Could not announce BugLogin app update: {error}");
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                eprintln!("Could not check for BugLogin app updates: {error}");
                            }
                        }
                    }
                });
            }
            setup_system_tray(app.handle()).map_err(|error| error.to_string())?;
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let _ = app.deep_link().register_all();
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let _ = handle.emit("buglogin://deep-link", url.to_string());
                }
            });

            if let Some(window) = app.get_webview_window("main") {
                let icon_bytes = include_bytes!("../icons/icon.png");
                if let Ok(image) = image::load_from_memory(icon_bytes) {
                    let rgba = image.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    let icon = tauri::image::Image::new_owned(rgba.into_raw(), width, height);
                    let _ = window.set_icon(icon);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system_health_get,
            integrations_status_get,
            api_tokens_list,
            api_token_create,
            api_token_revoke,
            get_app_settings,
            save_app_settings,
            get_system_language,
            get_system_info,
            get_table_sorting_settings,
            save_table_sorting_settings,
            get_table_preferences,
            save_table_preferences,
            get_onboarding_completed,
            complete_onboarding,
            get_window_resize_warning_dismissed,
            dismiss_window_resize_warning,
            runtime_catalog_get,
            runtime_catalog_current_device_get,
            runtime_installed_versions_get,
            check_wayfern_terms_accepted,
            accept_wayfern_terms,
            runtime_install,
            account_get,
            account_list,
            account_create,
            account_status_update,
            events_stream,
            events_stream_stop,
            organization_list,
            organization_get,
            organization_record_get,
            organization_create,
            organization_update,
            workspace_list,
            workspace_get,
            workspace_record_get,
            workspace_create,
            workspace_update,
            team_list,
            team_get,
            team_create,
            team_update,
            resource_list,
            resource_get,
            resource_lease_get,
            profile_runtime_get,
            get_runtime_device_id,
            resource_register,
            group_list,
            group_get,
            group_create,
            group_update,
            group_delete,
            extension_group_list,
            extension_group_get,
            extension_group_create,
            extension_group_update,
            extension_group_delete,
            profile_extension_group_set,
            list_extension_groups,
            create_extension_group,
            update_extension_group,
            delete_extension_group,
            set_extension_group_sync_enabled,
            assign_extension_group_to_profile,
            get_extension_group_for_profile,
            group_share_list,
            group_share_create,
            group_share_revoke,
            member_list,
            member_get,
            member_invite,
            member_invitation_create,
            member_invitation_redeem,
            member_invitation_revoke,
            member_activate,
            member_suspend,
            member_role_update,
            profile_groups_replace,
            operation_get,
            migration_inspect,
            migration_import,
            migration_profile_data_import,
            profile_list,
            profile_get,
            profile_binding_get,
            profile_proxy_assignment_get,
            profile_update,
            profile_proxy_assignment_set,
            profile_keyrotation_recipients_list,
            profile_keyrotation_execute,
            profile_create,
            profile_delete,
            profile_archive,
            profile_restore,
            profile_clone,
            profile_share,
            profile_share_revoke,
            profile_share_recipientdevicekeys_list,
            profile_launch,
            profile_stop,
            profile_sync_recovery_code_get,
            profile_sync_recover,
            cookie_list,
            cookie_stage,
            cookie_import,
            cookie_copy,
            proxy_parse,
            proxy_import,
            proxy_check,
            proxy_list,
            proxy_get,
            proxy_create,
            proxy_update,
            proxy_delete,
            grant_list,
            grant_list_page,
            grant_create,
            grant_revoke,
            list_browser_profiles,
            parse_txt_proxies,
            get_stored_proxies,
            get_available_workspaces,
            select_active_workspace,
            create_stored_proxy,
            update_stored_proxy,
            delete_stored_proxy,
            check_proxy_validity,
            check_stored_proxy,
            get_groups,
            get_groups_with_profile_counts,
            get_browser_profiles,
            get_profile_groups,
            create_profile_group,
            update_profile_group,
            delete_profile_group,
            assign_profiles_to_group,
            update_profile_proxy,
            update_profile_note,
            update_profile_tags,
            update_wayfern_config,
            rename_profile,
            delete_profile,
            delete_selected_profiles,
            import_proxies_from_parsed,
            import_proxies_json,
            create_browser_profile_new,
            launch_browser_profile,
            kill_browser_profile,
            get_all_traffic_snapshots,
            check_browser_status,
            list_running_browser_profiles,
            desktop_auth_status,
            desktop_auth_login,
            desktop_auth_sso_start,
            desktop_auth_sso_complete,
            desktop_deep_link_current,
            desktop_auth_logout,
            get_profile_traffic_snapshot,
            update_profile_clear_on_close,
            update_profile_window_color,
            update_profile_launch_hook,
            update_profile_start_urls,
            set_profile_sync_mode,
            update_profile_dns_server,
            update_profile_dns_blocklist,
            update_profile_proxy_bypass_rules,
            verify_profile_password,
            set_profile_password,
            change_profile_password,
            remove_profile_password,
            get_profile_cookie_stats,
            export_profile_cookies,
            import_profile_cookies,
            get_profile_local_storage_stats,
            export_profile_storage_state,
            import_profile_storage_state,
            hide_to_tray,
            // Profile lifecycle & trash
            profile_lifecycle::clone_profile,
            profile_lifecycle::open_url_with_profile,
            profile_lifecycle::is_profile_locked,
            profile_lifecycle::unlock_profile,
            profile_lifecycle::check_profile_fingerprint_consistency,
            profile_lifecycle::match_profile_fingerprint_to_exit,
            profile_lifecycle::list_trashed_profiles,
            profile_lifecycle::restore_trashed_profile,
            profile_lifecycle::purge_trashed_profile,
            profile_lifecycle::empty_trash,
            profile_lifecycle::export_profile,
            profile_lifecycle::preview_profile_archive,
            profile_lifecycle::import_profile_archive,
            // Cookie tools
            cookie_tools::read_profile_cookies,
            cookie_tools::copy_profile_cookies,
            cookie_tools::analyze_pasted_cookies,
            cookie_tools::import_pasted_cookies,
            // Network & DNS & VPN & Proxies
            network_features::get_dns_blocklist_cache_status,
            network_features::refresh_dns_blocklists,
            network_features::get_custom_dns_config,
            network_features::set_custom_dns_config,
            network_features::import_custom_dns_rules,
            network_features::export_custom_dns_rules,
            network_features::describe_dns_server,
            network_features::list_vpn_configs,
            network_features::get_vpn_config,
            network_features::import_vpn_config,
            network_features::create_vpn_config_manual,
            network_features::update_vpn_config,
            network_features::delete_vpn_config,
            network_features::check_vpn_validity,
            network_features::connect_vpn,
            network_features::disconnect_vpn,
            network_features::get_vpn_status,
            network_features::get_vpn_connection_status,
            network_features::is_vpn_in_use_by_synced_profile,
            network_features::set_vpn_sync_enabled,
            network_features::plan_proxy_distribution,
            network_features::distribute_proxies_to_profiles,
            network_features::export_proxies,
            network_features::rotate_proxy_ip,
            network_features::rotate_proxy_ips,
            network_features::get_cached_proxy_check,
            network_features::get_proxy_check_history,
            network_features::cloud_get_countries,
            network_features::cloud_get_regions,
            network_features::cloud_get_cities,
            network_features::cloud_get_isps,
            network_features::create_cloud_location_proxy,
            // Extensions
            extension_features::list_extensions,
            extension_features::add_extension,
            extension_features::add_unpacked_extension,
            extension_features::update_extension,
            extension_features::update_extension_from_path,
            extension_features::delete_extension,
            extension_features::fetch_extension_from_url,
            extension_features::get_extension_icon,
            extension_features::set_extension_sync_enabled,
            // Sync & Restore points & Synchronizer
            sync_features::list_restore_points,
            sync_features::create_restore_point,
            sync_features::delete_restore_point,
            sync_features::restore_restore_point,
            sync_features::cloud_get_user,
            sync_features::cloud_exchange_device_code,
            sync_features::cloud_logout,
            sync_features::cloud_refresh_profile,
            sync_features::cloud_get_proxy_usage,
            sync_features::get_sync_settings,
            sync_features::save_sync_settings,
            sync_features::request_profile_sync,
            sync_features::cancel_profile_sync,
            sync_features::get_unsynced_entity_counts,
            sync_features::enable_sync_for_all_entities,
            sync_features::check_sync_server_connection,
            sync_features::restart_sync_service,
            sync_features::check_has_e2e_password,
            sync_features::set_e2e_password,
            sync_features::verify_e2e_password,
            sync_features::delete_e2e_password,
            sync_features::rollover_encryption_for_all_entities,
            sync_features::start_sync_session,
            sync_features::stop_sync_session,
            sync_features::set_sync_session_paused,
            sync_features::arrange_sync_windows,
            sync_features::set_sync_follower_held,
            sync_features::remove_sync_follower,
            // System, Updates, Logs, TOTP & Bookmarks
            system_features::check_for_app_updates,
            system_features::check_for_app_updates_manual,
            system_features::download_and_prepare_app_update,
            system_features::restart_application,
            system_features::check_browser_exists,
            system_features::download_browser,
            system_features::check_for_browser_updates,
            system_features::get_version_update_status,
            system_features::trigger_manual_version_update,
            system_features::fetch_browser_versions_with_count,
            system_features::complete_browser_update_with_auto_update,
            system_features::dismiss_update_notification,
            system_features::clear_all_version_cache_and_refetch,
            system_features::read_log_files,
            system_features::open_log_directory,
            system_features::preview_feedback_logs,
            system_features::send_feedback,
            system_features::get_tips_state,
            system_features::mark_tip_seen,
            system_features::set_tips_auto_show,
            system_features::confirm_quit,
            system_features::update_tray_menu,
            system_features::get_window_decoration_layout,
            system_features::get_audit_events,
            system_features::check_integration_connection,
            system_features::clear_all_traffic_stats,
            system_features::clear_profile_traffic_stats,
            system_features::get_traffic_stats_for_period,
            system_features::get_commercial_trial_status,
            system_features::has_acknowledged_trial_expiration,
            system_features::generate_totp_code,
            system_features::list_profile_totp,
            system_features::get_profile_totp_codes,
            system_features::add_profile_totp,
            system_features::delete_profile_totp,
            system_features::get_group_bookmarks,
            system_features::set_group_bookmarks,
            system_features::apply_group_bookmarks_to_profile,
            system_features::get_team_locks,
            system_features::list_team_activity,
            system_features::export_team_activity_csv,
            system_features::list_team_members,
            system_features::set_team_member_access,
            // Agent Console, Cookie Bot, Recipes, Remote Sessions & Schedules
            agent_features::get_agent_console,
            agent_features::answer_agent_request,
            agent_features::dismiss_agent_request,
            agent_features::send_agent_note,
            agent_features::take_over_profile,
            agent_features::hand_back_profile,
            agent_features::set_agents_paused,
            agent_features::clear_agent_activity,
            agent_features::show_profile_window,
            agent_features::get_cookie_bot_schedules,
            agent_features::get_cookie_bot_schedule,
            agent_features::save_cookie_bot_schedule,
            agent_features::delete_cookie_bot_schedule,
            agent_features::check_cookie_bot_conflicts,
            agent_features::get_cookie_bot_runs,
            agent_features::run_cookie_bot_now,
            agent_features::cancel_cookie_bot_run,
            agent_features::get_cookie_bot_presets,
            agent_features::get_cookie_bot_user_templates,
            agent_features::create_cookie_bot_user_template,
            agent_features::update_cookie_bot_user_template,
            agent_features::delete_cookie_bot_user_template,
            agent_features::get_remote_hours_quota,
            agent_features::get_cookie_bot_usage,
            agent_features::get_agent_recipes,
            agent_features::create_agent_recipe,
            agent_features::update_agent_recipe,
            agent_features::delete_agent_recipe,
            agent_features::get_recipe_recording,
            agent_features::start_recipe_recording,
            agent_features::stop_recipe_recording,
            agent_features::list_remote_sessions,
            agent_features::get_remote_session,
            agent_features::stop_remote_session,
            agent_features::start_remote_session_events,
            agent_features::stop_remote_session_events,
            agent_features::get_remote_session_events_status,
            agent_features::list_agent_schedules,
            agent_features::create_agent_schedule,
            agent_features::update_agent_schedule,
            agent_features::delete_agent_schedule,
            agent_features::run_agent_schedule_now,
            agent_features::test_agent_schedule_notice,
            agent_features::list_agent_schedule_runs,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build BugLogin desktop application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if let Some(state) = app.try_state::<DesktopState>() {
                    if tauri::async_runtime::block_on(state.shutdown()).is_err() {
                        api.prevent_exit();
                    }
                }
            }
        });
}

#[tauri::command]
fn hide_to_tray(app_handle: tauri::AppHandle) -> Result<(), String> {
    app_handle
        .get_webview_window("main")
        .ok_or_else(|| "main_window_not_found".to_owned())?
        .hide()
        .map_err(|error| error.to_string())
}

fn show_main_window(app_handle: &tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_system_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItemBuilder::with_id("tray_show", "Show BugLogin").build(app)?;
    let quit = MenuItemBuilder::with_id("tray_quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&quit)
        .build()?;
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../icons/tray-icon-44.png");
    #[cfg(target_os = "windows")]
    let bytes = include_bytes!("../icons/tray-icon-win-44.png");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let bytes = include_bytes!("../icons/tray-icon-linux-44.png");
    let image = image::load_from_memory(bytes)?.into_rgba8();
    let (width, height) = image.dimensions();
    let icon = tauri::image::Image::new_owned(image.into_raw(), width, height);
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("BugLogin")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray_show" => show_main_window(app),
            "tray_quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[tauri::command]
async fn desktop_auth_status(state: State<'_, DesktopState>) -> Result<DesktopAuthStatus, Problem> {
    state.restore_auth("desktop-auth-status").await?;
    state.auth_status()
}

#[tauri::command]
async fn runtime_catalog_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<buglogin_browser::runtime::catalog::RuntimeCatalog, Problem> {
    state.runtime_catalog_get(&correlation_id).await
}

#[tauri::command]
async fn runtime_catalog_current_device_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<buglogin_browser::runtime::catalog::RuntimeCatalog, Problem> {
    let (platform, architecture) = runtime_install::current_platform_architecture(&correlation_id)?;
    let mut catalog = state.runtime_catalog_get(&correlation_id).await?;
    catalog
        .releases
        .retain(|release| release.platform == platform && release.architecture == architecture);
    Ok(catalog)
}

#[tauri::command]
fn runtime_installed_versions_get(
    state: State<'_, DesktopState>,
    runtime: buglogin_domain::entities::Runtime,
    correlation_id: String,
) -> Result<Vec<String>, Problem> {
    state.runtime_installed_versions_get(&runtime, &correlation_id)
}

#[tauri::command]
fn check_wayfern_terms_accepted() -> bool {
    wayfern_terms::is_terms_accepted()
}

#[tauri::command]
fn accept_wayfern_terms(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<(), Problem> {
    let executable = state.wayfern_executable_path_get(&correlation_id)?;
    wayfern_terms::accept_terms(&executable, &correlation_id)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeInstallResult {
    runtime: String,
    version: String,
    executable_path: String,
}

#[tauri::command]
async fn runtime_install(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    runtime: buglogin_domain::entities::Runtime,
    version: String,
    correlation_id: String,
) -> Result<RuntimeInstallResult, Problem> {
    if version.trim().is_empty() {
        return Err(Problem::new(
            "runtime_version_required",
            "A browser runtime version is required",
            400,
            &correlation_id,
        ));
    }
    let installed_target = install_verified_runtime(
        app.clone(),
        state.inner(),
        runtime.clone(),
        &version,
        &correlation_id,
    )
    .await?;
    let result = RuntimeInstallResult {
        runtime: runtime_name(&runtime).to_owned(),
        version,
        executable_path: installed_target.binary.path.to_string_lossy().into_owned(),
    };
    Ok(result)
}

fn runtime_name(runtime: &buglogin_domain::entities::Runtime) -> &'static str {
    use buglogin_domain::entities::Runtime;
    match runtime {
        Runtime::Wayfern => "wayfern",
        Runtime::Bugmini => "bugmini",
        _ => "unsupported",
    }
}

async fn install_verified_runtime(
    app: tauri::AppHandle,
    state: &DesktopState,
    runtime: buglogin_domain::entities::Runtime,
    version: &str,
    correlation_id: &str,
) -> Result<adapter::RuntimeTarget, Problem> {
    use buglogin_domain::entities::Runtime;

    let (platform, architecture) = runtime_install::current_platform_architecture(correlation_id)?;
    if !matches!(&runtime, Runtime::Wayfern | Runtime::Bugmini) {
        return Err(Problem::new(
            "runtime_not_supported",
            "Runtime has no approved desktop adapter",
            400,
            correlation_id,
        ));
    }
    let catalog = state.runtime_catalog_get(correlation_id).await?;
    let verifier = runtime_config::pinned_runtime_verifier(correlation_id)?;
    let releases = catalog
        .verify(&verifier)
        .map_err(|error| Problem::new(error.code, error.title, error.status, correlation_id))?;
    let release = releases
        .into_iter()
        .find(|candidate| {
            let release = candidate.release();
            release.runtime == runtime
                && release.version == version
                && release.platform == platform
                && release.architecture == architecture
        })
        .ok_or_else(|| {
            Problem::new(
                "runtime_release_not_found",
                "The requested signed browser runtime release is unavailable",
                404,
                correlation_id,
            )
        })?;
    let runtime_root = runtime_config::runtime_install_root()?;
    let package_size = release.release().size_bytes;
    let catalog_schema_version = catalog.schema_version;
    let install_app = app.clone();
    let installed_target = tauri::async_runtime::spawn_blocking(move || {
        runtime_install::install(
            &runtime_root,
            &release,
            catalog_schema_version,
            &install_app,
        )
    })
    .await
    .map_err(|_| {
        Problem::new(
            "runtime_install_failed",
            "Browser runtime installation failed",
            500,
            correlation_id,
        )
    })??;
    if let Err(error) = state
        .register_runtime_target(runtime.clone(), installed_target.clone(), correlation_id)
        .await
    {
        runtime_install::emit_progress(
            &app,
            runtime_name(&runtime),
            version,
            0,
            package_size,
            0,
            None,
            "error",
        );
        return Err(error);
    }
    runtime_install::emit_progress(
        &app,
        runtime_name(&runtime),
        version,
        package_size,
        package_size,
        0,
        None,
        "completed",
    );
    Ok(installed_target)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileRuntimeSelection {
    runtime: buglogin_domain::entities::Runtime,
    runtime_version: Option<String>,
}

async fn ensure_profile_runtime_installed(
    app: tauri::AppHandle,
    state: &DesktopState,
    profile_id: &str,
    correlation_id: &str,
) -> Result<(), Problem> {
    use buglogin_domain::entities::Runtime;

    let profile = state
        .invoke(
            "profile_get",
            correlation_id,
            None,
            json!({ "profileId": profile_id }),
        )
        .await?;
    let selection: ProfileRuntimeSelection =
        serde_json::from_value(profile.data).map_err(|_| {
            Problem::new(
                "profile_runtime_invalid",
                "Profile browser runtime settings are invalid",
                502,
                correlation_id,
            )
        })?;
    if !matches!(selection.runtime, Runtime::Wayfern | Runtime::Bugmini) {
        return Ok(());
    }

    let installed_versions =
        state.runtime_installed_versions_get(&selection.runtime, correlation_id)?;
    if selection
        .runtime_version
        .as_ref()
        .map_or(!installed_versions.is_empty(), |version| {
            installed_versions
                .iter()
                .any(|installed| installed == version)
        })
    {
        return Ok(());
    }
    if matches!(state, DesktopState::Local(_)) {
        return Err(Problem::new(
            "runtime_install_hosted_only",
            "Verified browser runtimes require BugLogin hosted authority",
            409,
            correlation_id,
        ));
    }

    let (platform, architecture) = runtime_install::current_platform_architecture(correlation_id)?;
    let catalog = state.runtime_catalog_get(correlation_id).await?;
    let verifier = runtime_config::pinned_runtime_verifier(correlation_id)?;
    let releases = catalog
        .verify(&verifier)
        .map_err(|error| Problem::new(error.code, error.title, error.status, correlation_id))?;
    let release_index: Vec<_> = releases
        .iter()
        .map(|candidate| {
            let release = candidate.release();
            (
                release.runtime.clone(),
                release.version.clone(),
                release.platform.clone(),
                release.architecture.clone(),
            )
        })
        .collect();
    let Some(requested_version) = runtime_version_to_install(
        &selection.runtime,
        selection.runtime_version.as_deref(),
        &installed_versions,
        &release_index,
        &platform,
        &architecture,
        correlation_id,
    )?
    else {
        return Ok(());
    };

    install_verified_runtime(
        app,
        state,
        selection.runtime,
        &requested_version,
        correlation_id,
    )
    .await?;
    Ok(())
}

fn runtime_version_to_install(
    runtime: &buglogin_domain::entities::Runtime,
    selected_version: Option<&str>,
    installed_versions: &[String],
    releases: &[(
        buglogin_domain::entities::Runtime,
        String,
        buglogin_domain::entities::RuntimePlatform,
        buglogin_domain::entities::RuntimeArchitecture,
    )],
    platform: &buglogin_domain::entities::RuntimePlatform,
    architecture: &buglogin_domain::entities::RuntimeArchitecture,
    correlation_id: &str,
) -> Result<Option<String>, Problem> {
    if let Some(version) = selected_version {
        if installed_versions
            .iter()
            .any(|installed| installed == version)
        {
            return Ok(None);
        }
        if releases.iter().any(
            |(release_runtime, release_version, release_platform, release_arch)| {
                release_runtime == runtime
                    && release_version == version
                    && release_platform == platform
                    && release_arch == architecture
            },
        ) {
            return Ok(Some(version.to_owned()));
        }
        return Err(Problem::new(
            "runtime_release_not_found",
            "The selected signed browser release is unavailable for this device",
            404,
            correlation_id,
        ));
    }
    if !installed_versions.is_empty() {
        return Ok(None);
    }
    releases
        .iter()
        .filter_map(
            |(release_runtime, version, release_platform, release_arch)| {
                (release_runtime == runtime
                    && release_platform == platform
                    && release_arch == architecture)
                    .then_some(version.as_str())
            },
        )
        .max_by(|left, right| compare_numeric_versions(left, right))
        .map(|version| Some(version.to_owned()))
        .ok_or_else(|| {
            Problem::new(
                "runtime_release_not_found",
                "No signed browser release is available for this device",
                404,
                correlation_id,
            )
        })
}

#[cfg(test)]
mod runtime_version_selection_tests {
    use super::runtime_version_to_install;
    use buglogin_domain::entities::{Runtime, RuntimeArchitecture, RuntimePlatform};

    fn catalog_releases(
        platform: RuntimePlatform,
        architecture: RuntimeArchitecture,
    ) -> Vec<(Runtime, String, RuntimePlatform, RuntimeArchitecture)> {
        ["154.0.8037.57", "155.0.8059.79"]
            .into_iter()
            .map(|version| {
                (
                    Runtime::Wayfern,
                    version.to_owned(),
                    platform.clone(),
                    architecture.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn first_launch_without_any_installed_version_selects_latest_compatible_catalog_release() {
        let requested = runtime_version_to_install(
            &Runtime::Wayfern,
            None,
            &[],
            &catalog_releases(RuntimePlatform::Windows, RuntimeArchitecture::X64),
            &RuntimePlatform::Windows,
            &RuntimeArchitecture::X64,
            "runtime-version-test",
        )
        .unwrap();
        assert_eq!(requested.as_deref(), Some("155.0.8059.79"));
    }

    #[test]
    fn switching_to_an_uninstalled_selected_version_installs_that_exact_catalog_release() {
        let requested = runtime_version_to_install(
            &Runtime::Wayfern,
            Some("154.0.8037.57"),
            &["155.0.8059.79".to_owned()],
            &catalog_releases(RuntimePlatform::Windows, RuntimeArchitecture::X64),
            &RuntimePlatform::Windows,
            &RuntimeArchitecture::X64,
            "runtime-version-test",
        )
        .unwrap();
        assert_eq!(requested.as_deref(), Some("154.0.8037.57"));
    }

    #[test]
    fn already_installed_selected_version_skips_download() {
        let requested = runtime_version_to_install(
            &Runtime::Wayfern,
            Some("154.0.8037.57"),
            &["154.0.8037.57".to_owned()],
            &catalog_releases(RuntimePlatform::Windows, RuntimeArchitecture::X64),
            &RuntimePlatform::Windows,
            &RuntimeArchitecture::X64,
            "runtime-version-test",
        )
        .unwrap();
        assert_eq!(requested, None);
    }

    #[test]
    fn first_macos_arm64_launch_selects_latest_compatible_release() {
        let requested = runtime_version_to_install(
            &Runtime::Wayfern,
            None,
            &[],
            &catalog_releases(RuntimePlatform::Macos, RuntimeArchitecture::Arm64),
            &RuntimePlatform::Macos,
            &RuntimeArchitecture::Arm64,
            "runtime-version-test",
        )
        .unwrap();
        assert_eq!(requested.as_deref(), Some("155.0.8059.79"));
    }

    #[test]
    fn macos_version_switch_selects_the_exact_uninstalled_release() {
        let requested = runtime_version_to_install(
            &Runtime::Wayfern,
            Some("154.0.8037.57"),
            &["155.0.8059.79".to_owned()],
            &catalog_releases(RuntimePlatform::Macos, RuntimeArchitecture::Arm64),
            &RuntimePlatform::Macos,
            &RuntimeArchitecture::Arm64,
            "runtime-version-test",
        )
        .unwrap();
        assert_eq!(requested.as_deref(), Some("154.0.8037.57"));
    }

    #[test]
    fn macos_runtime_selection_never_uses_a_release_for_another_platform_or_architecture() {
        let error = runtime_version_to_install(
            &Runtime::Wayfern,
            None,
            &[],
            &catalog_releases(RuntimePlatform::Macos, RuntimeArchitecture::Arm64),
            &RuntimePlatform::Macos,
            &RuntimeArchitecture::X64,
            "runtime-version-test",
        )
        .unwrap_err();
        assert_eq!(error.code, "runtime_release_not_found");
    }
}

fn compare_numeric_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left: Vec<_> = left
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    let right: Vec<_> = right
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    for index in 0..left.len().max(right.len()) {
        match left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default())
        {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

#[tauri::command]
async fn desktop_auth_login(
    state: State<'_, DesktopState>,
    identifier: String,
    password: String,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state
        .login_with_credentials(
            Zeroizing::new(identifier),
            Zeroizing::new(password),
            &correlation_id,
        )
        .await
}

#[tauri::command]
async fn desktop_auth_sso_start(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<u64, Problem> {
    let response = state.start_buglab_sso(&correlation_id).await?;
    app.opener()
        .open_url(response.authorization_url, None::<&str>)
        .map_err(|_| {
            Problem::new(
                "sso_browser_open_failed",
                "Could not open BugLab ID sign-in page",
                503,
                &correlation_id,
            )
        })?;
    Ok(response.expires_in)
}

#[tauri::command]
async fn desktop_auth_sso_complete(
    state: State<'_, DesktopState>,
    callback_state: String,
    token: String,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state
        .complete_buglab_sso(
            Zeroizing::new(callback_state),
            Zeroizing::new(token),
            &correlation_id,
        )
        .await
}

#[tauri::command]
fn desktop_deep_link_current(app: tauri::AppHandle) -> Result<Vec<String>, Problem> {
    app.deep_link()
        .get_current()
        .map(|urls| {
            urls.unwrap_or_default()
                .into_iter()
                .map(|url| url.to_string())
                .collect()
        })
        .map_err(|_| {
            Problem::new(
                "deep_link_unavailable",
                "Could not read the sign-in callback",
                503,
                "desktop-deep-link",
            )
        })
}

#[tauri::command]
async fn desktop_auth_logout(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state.logout(&correlation_id).await
}

fn build_state() -> Result<DesktopState, Problem> {
    let config = runtime_config::open()?;
    match config.mode {
        runtime_config::DeploymentMode::Local => {
            let repository_root = config.repository_root.ok_or_else(|| {
                Problem::new(
                    "local_repository_required",
                    "Local repository root is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?;
            Ok(DesktopState::local(
                AdapterState::open_with_targets_and_migration(
                    repository_root,
                    config.profiles_root,
                    config.runtime_targets,
                    runtime_config::migration_executable(),
                )?,
            ))
        }
        runtime_config::DeploymentMode::Production => Ok(DesktopState::production(
            config.api_base_url.as_deref().ok_or_else(|| {
                Problem::new(
                    "production_api_required",
                    "Production API endpoint is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?,
            config.api_token,
            config.device_id.ok_or_else(|| {
                Problem::new(
                    "production_device_id_required",
                    "Production device ID is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?,
            config.profiles_root,
            config.runtime_targets,
        )?),
    }
}

#[tauri::command]
async fn system_health_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("system_health_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn integrations_status_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<desktop_state::DesktopIntegrationStatus, Problem> {
    state.integration_status(&correlation_id).await
}

#[tauri::command]
async fn api_tokens_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<remote_api::ApiTokenPage, Problem> {
    state.list_api_tokens(&correlation_id).await
}

#[tauri::command]
async fn api_token_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    name: String,
    scopes: std::collections::BTreeSet<String>,
    expires_at: u64,
) -> Result<remote_api::CreatedApiToken, Problem> {
    state
        .create_api_token(&name, scopes, expires_at, &correlation_id)
        .await
}

#[tauri::command]
async fn api_token_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    token_id: String,
) -> Result<(), Problem> {
    state.revoke_api_token(&token_id, &correlation_id).await
}

#[tauri::command]
fn get_system_info() -> Value {
    json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "portable": false,
    })
}

#[tauri::command]
async fn events_stream(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    streams: State<'_, event_stream::EventStreamManager>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    let (workspace_id, after, limit) = event_stream::parse_input(payload, &correlation_id)?;
    let client = state.event_stream_client(&correlation_id).await?;
    let subscription_id = streams.start(
        app,
        client,
        workspace_id.clone(),
        after,
        limit,
        correlation_id.clone(),
    )?;
    Ok(CommandResult {
        command_id: "events.stream".to_owned(),
        correlation_id,
        data: json!({"workspaceId": workspace_id.as_str(), "subscriptionId": subscription_id, "active": true}),
    })
}

#[tauri::command]
fn events_stream_stop(
    streams: State<'_, event_stream::EventStreamManager>,
    subscription_id: String,
) -> Result<(), Problem> {
    streams.stop(&subscription_id)
}

#[tauri::command]
async fn account_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("account_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn account_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("account_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn account_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "account_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn account_status_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "account_status_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn organization_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("organization_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn organization_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("organization_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn organization_record_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("organization_record_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn organization_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "organization_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn organization_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "organization_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn workspace_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("workspace_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn workspace_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("workspace_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn workspace_record_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("workspace_record_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn workspace_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "workspace_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn workspace_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "workspace_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn team_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("team_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn team_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("team_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn team_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "team_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn team_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "team_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn resource_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("resource_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn resource_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("resource_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn resource_lease_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("resource_lease_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn profile_runtime_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_runtime_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
fn get_runtime_device_id(state: State<'_, DesktopState>) -> Option<String> {
    state.runtime_device_id()
}

#[tauri::command]
async fn resource_register(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "resource_register",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn group_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("group_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn group_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("group_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn group_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "group_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn group_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "group_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn group_delete(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "group_delete",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn extension_group_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("extension_group_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn extension_group_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("extension_group_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn extension_group_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "extension_group_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn extension_group_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "extension_group_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn extension_group_delete(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "extension_group_delete",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_extension_group_set(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_extension_group_set",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

fn extension_command_id(prefix: &str) -> Result<String, Problem> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        Problem::new(
            "random_source_unavailable",
            "Secure random source unavailable",
            503,
            "desktop",
        )
    })?;
    let suffix = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{prefix}-{suffix}"))
}

fn extension_group_ui_value(mut group: Value) -> Value {
    if let Some(object) = group.as_object_mut() {
        if let Some(extension_ids) = object.get("extensionIds").cloned() {
            object.insert("extension_ids".into(), extension_ids);
        }
        let sync_enabled = object.get("syncMode").and_then(Value::as_str) == Some("managed");
        object.insert("sync_enabled".into(), Value::Bool(sync_enabled));
    }
    group
}

#[tauri::command]
async fn list_extension_groups(
    state: State<'_, DesktopState>,
    workspace_id: String,
) -> Result<Vec<Value>, Problem> {
    let result = state
        .invoke(
            "extension_group_list",
            &extension_command_id("extension-group-list")?,
            None,
            json!({ "workspaceId": workspace_id }),
        )
        .await?;
    let rows = result.data.as_array().cloned().unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter_map(|row| row.get("group").cloned())
        .map(extension_group_ui_value)
        .collect())
}

#[tauri::command]
async fn create_extension_group(
    state: State<'_, DesktopState>,
    workspace_id: String,
    name: String,
) -> Result<Value, Problem> {
    let result = state.invoke(
        "extension_group_create",
        &extension_command_id("extension-group-create")?,
        Some(&extension_command_id("extension-group-create-key")?),
        json!({ "workspaceId": workspace_id, "extensionGroupId": extension_command_id("extgrp")?, "name": name, "extensionIds": [], "syncMode": "local" }),
    ).await?;
    Ok(extension_group_ui_value(
        result.data.get("group").cloned().unwrap_or(Value::Null),
    ))
}

#[tauri::command]
async fn update_extension_group(
    state: State<'_, DesktopState>,
    workspace_id: String,
    group_id: String,
    name: String,
    extension_ids: Option<Vec<String>>,
) -> Result<Value, Problem> {
    let lookup = state
        .invoke(
            "extension_group_get",
            &extension_command_id("extension-group-get")?,
            None,
            json!({ "workspaceId": workspace_id, "extensionGroupId": group_id }),
        )
        .await?;
    let revision = lookup
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let group = lookup.data.get("group").cloned().unwrap_or(Value::Null);
    let extension_ids = extension_ids.map(Value::from).unwrap_or_else(|| {
        group
            .get("extensionIds")
            .cloned()
            .unwrap_or_else(|| json!([]))
    });
    let sync_mode = group.get("syncMode").cloned().unwrap_or(json!("local"));
    let result = state.invoke(
        "extension_group_update", &extension_command_id("extension-group-update")?,
        Some(&extension_command_id("extension-group-update-key")?),
        json!({ "workspaceId": workspace_id, "extensionGroupId": group_id, "expectedRevision": revision, "name": name, "extensionIds": extension_ids, "syncMode": sync_mode }),
    ).await?;
    Ok(extension_group_ui_value(
        result.data.get("group").cloned().unwrap_or(Value::Null),
    ))
}

#[tauri::command]
async fn delete_extension_group(
    state: State<'_, DesktopState>,
    workspace_id: String,
    group_id: String,
) -> Result<Value, Problem> {
    let lookup = state
        .invoke(
            "extension_group_get",
            &extension_command_id("extension-group-get")?,
            None,
            json!({ "workspaceId": workspace_id, "extensionGroupId": group_id }),
        )
        .await?;
    let revision = lookup
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let result = state.invoke(
        "extension_group_delete", &extension_command_id("extension-group-delete")?,
        Some(&extension_command_id("extension-group-delete-key")?),
        json!({ "workspaceId": workspace_id, "extensionGroupId": group_id, "expectedRevision": revision }),
    ).await?;
    Ok(extension_group_ui_value(
        result.data.get("group").cloned().unwrap_or(Value::Null),
    ))
}

#[tauri::command]
async fn set_extension_group_sync_enabled(
    state: State<'_, DesktopState>,
    workspace_id: String,
    group_id: String,
    enabled: bool,
) -> Result<Value, Problem> {
    let lookup = state
        .invoke(
            "extension_group_get",
            &extension_command_id("extension-group-get")?,
            None,
            json!({ "workspaceId": workspace_id, "extensionGroupId": group_id }),
        )
        .await?;
    let revision = lookup
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let group = lookup.data.get("group").cloned().unwrap_or(Value::Null);
    let name = group
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let extension_ids = group
        .get("extensionIds")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let result = state
        .invoke(
            "extension_group_update",
            &extension_command_id("extension-group-update")?,
            Some(&extension_command_id("extension-group-update-key")?),
            json!({
                "workspaceId": workspace_id,
                "extensionGroupId": group_id,
                "expectedRevision": revision,
                "name": name,
                "extensionIds": extension_ids,
                "syncMode": if enabled { "managed" } else { "local" }
            }),
        )
        .await?;
    Ok(extension_group_ui_value(
        result.data.get("group").cloned().unwrap_or(Value::Null),
    ))
}

#[tauri::command]
async fn assign_extension_group_to_profile(
    state: State<'_, DesktopState>,
    workspace_id: String,
    profile_id: String,
    extension_group_id: Option<String>,
) -> Result<Value, Problem> {
    let lookup = state
        .invoke(
            "profile_get",
            &extension_command_id("profile-get")?,
            None,
            json!({ "profileId": profile_id }),
        )
        .await?;
    let revision = lookup
        .data
        .get("revision")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let result = state.invoke(
        "profile_extension_group_set", &extension_command_id("profile-extension-group-set")?,
        Some(&extension_command_id("profile-extension-group-set-key")?),
        json!({ "workspaceId": workspace_id, "profileId": profile_id, "expectedRevision": revision, "extensionGroupId": extension_group_id }),
    ).await?;
    Ok(result.data)
}

#[tauri::command]
async fn get_extension_group_for_profile(
    state: State<'_, DesktopState>,
    workspace_id: String,
    profile_id: String,
) -> Result<Value, Problem> {
    let profile = state
        .invoke(
            "profile_get",
            &extension_command_id("profile-get")?,
            None,
            json!({ "profileId": profile_id }),
        )
        .await?;
    let Some(group_id) = profile.data.get("extensionGroupId").and_then(Value::as_str) else {
        return Ok(Value::Null);
    };
    let result = state
        .invoke(
            "extension_group_get",
            &extension_command_id("extension-group-get")?,
            None,
            json!({ "workspaceId": workspace_id, "extensionGroupId": group_id }),
        )
        .await?;
    Ok(extension_group_ui_value(
        result.data.get("group").cloned().unwrap_or(Value::Null),
    ))
}

#[tauri::command]
async fn group_share_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("group_share_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn group_share_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "group_share_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn group_share_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "group_share_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("member_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn member_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("member_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn member_invite(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_invite",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_invitation_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_invitation_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_invitation_redeem(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_invitation_redeem",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_invitation_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_invitation_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_activate(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_activate",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_suspend(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_suspend",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn member_role_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "member_role_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_groups_replace(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_groups_replace",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn operation_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("operation_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn migration_inspect(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "migration_inspect",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn migration_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "migration_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn migration_profile_data_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "migration_profile_data_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn profile_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn profile_delete(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_delete",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_binding_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_binding_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn profile_proxy_assignment_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_proxy_assignment_get",
            &correlation_id,
            None,
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_proxy_assignment_set(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_proxy_assignment_set",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_keyrotation_recipients_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_keyrotation_recipients_list",
            &correlation_id,
            None,
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_keyrotation_execute(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_keyrotation_execute",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_archive(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_archive",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_restore(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_restore",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_clone(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_clone",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_share(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_share",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_share_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_share_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_share_recipientdevicekeys_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_share_recipientdevicekeys_list",
            &correlation_id,
            None,
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_launch(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    if let Some(profile_id) = payload.get("profileId").and_then(Value::as_str) {
        ensure_profile_runtime_installed(app, state.inner(), profile_id, &correlation_id).await?;
    }
    state
        .invoke(
            "profile_launch",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_stop(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_stop",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_sync_recovery_code_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_sync_recovery_code_get",
            &correlation_id,
            None,
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_sync_recover(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_sync_recover", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn cookie_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("cookie_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn cookie_stage(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_stage",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn cookie_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn cookie_copy(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_copy",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_parse(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_parse", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_check(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_check", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_delete(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_delete",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn grant_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("grant_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn grant_list_page(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("grant_list_page", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn grant_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "grant_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn grant_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "grant_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}
