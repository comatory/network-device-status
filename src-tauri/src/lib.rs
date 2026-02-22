use smappservice_rs::{AppService, ServiceStatus, ServiceType};
use std::{
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use system_configuration::{
    core_foundation::{
        array::CFArray,
        base::TCFType,
        runloop::{kCFRunLoopDefaultMode, CFRunLoop},
        string::CFString,
    },
    dynamic_store::{SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext},
    network_configuration::SCNetworkService,
    preferences::SCPreferences,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle,
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_store::StoreExt;

// (CheckMenuItem, bsd_name)
type SharedItems = Arc<Mutex<Vec<(CheckMenuItem<tauri::Wry>, String)>>>;

fn is_launch_at_login_enabled() -> bool {
    AppService::new(ServiceType::MainApp).status() == ServiceStatus::Enabled
}

fn toggle_launch_at_login(enable: bool) -> bool {
    let service = AppService::new(ServiceType::MainApp);
    if enable {
        service.register().is_ok()
    } else {
        service.unregister().is_ok()
    }
}

fn load_show_notifications(app: &AppHandle) -> bool {
    if let Ok(store) = app.store("settings.json") {
        store
            .get("show_notifications")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    } else {
        true
    }
}

fn save_show_notifications(app: &AppHandle, value: bool) {
    if let Ok(store) = app.store("settings.json") {
        store.set("show_notifications", serde_json::Value::Bool(value));
        let _ = store.save();
    }
}

fn check_internet() -> bool {
    TcpStream::connect_timeout(
        &"1.1.1.1:53".parse::<std::net::SocketAddr>().unwrap(),
        Duration::from_secs(3),
    )
    .is_ok()
}

fn get_active_interface(store: &SCDynamicStore) -> Option<String> {
    use system_configuration::core_foundation::{base::CFType, dictionary::CFDictionary};

    let key = CFString::new("State:/Network/Global/IPv4");
    let plist = store.get(key)?;

    let dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(plist.as_concrete_TypeRef() as _) };

    let iface_key = CFString::new("PrimaryInterface");
    let value = dict.find(iface_key)?;
    value
        .downcast::<CFString>()
        .map(|s: CFString| s.to_string())
}

// Returns (display_name, bsd_name) for each TCP/IP-capable network service.
fn get_network_services() -> Vec<(String, String)> {
    let prefs = SCPreferences::default(&CFString::new("net-monitor"));
    SCNetworkService::get_services(&prefs)
        .iter()
        .filter_map(|svc| {
            let iface = svc.network_interface()?;
            let bsd = iface.bsd_name()?.to_string();
            let display = iface
                .display_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| bsd.clone());
            Some((display, bsd))
        })
        .collect()
}

fn get_display_name(bsd_name: &str) -> String {
    get_network_services()
        .into_iter()
        .find(|(_, bsd)| bsd == bsd_name)
        .map(|(display, _)| display)
        .unwrap_or_else(|| bsd_name.to_string())
}

fn create_dot_icon(connected: bool) -> Image<'static> {
    let size: u32 = 22;
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let radius = 6.0f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let idx = ((y * size + x) * 4) as usize;
            if dx * dx + dy * dy <= radius * radius {
                if connected {
                    rgba[idx] = 34;
                    rgba[idx + 1] = 197;
                    rgba[idx + 2] = 94;
                } else {
                    rgba[idx] = 60;
                    rgba[idx + 1] = 60;
                    rgba[idx + 2] = 60;
                }
                rgba[idx + 3] = 255;
            }
        }
    }
    Image::new_owned(rgba, size, size)
}

fn create_items(
    app: &AppHandle,
    services: &[(String, String)],
    active: Option<&str>,
) -> tauri::Result<Vec<(CheckMenuItem<tauri::Wry>, String)>> {
    services
        .iter()
        .map(|(display, bsd)| {
            let item = CheckMenuItem::with_id(
                app,
                bsd,
                display,
                true,
                Some(bsd.as_str()) == active,
                None::<&str>,
            )?;
            Ok((item, bsd.clone()))
        })
        .collect()
}

fn assemble_menu(
    app: &AppHandle,
    items: &[(CheckMenuItem<tauri::Wry>, String)],
    launch_at_login: bool,
    show_notifications: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let sep = PredefinedMenuItem::separator(app)?;
    let about = MenuItem::with_id(app, "about", "About", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let launch = CheckMenuItem::with_id(
        app,
        "launch_at_login",
        "Launch at Login",
        true,
        launch_at_login,
        None::<&str>,
    )?;
    let notify = CheckMenuItem::with_id(
        app,
        "show_notifications",
        "Show notification when interface changes",
        true,
        show_notifications,
        None::<&str>,
    )?;

    let settings_menu = Submenu::with_items(app, "Settings", true, &[&launch, &notify])?;

    let mut refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items
        .iter()
        .map(|(i, _)| i as &dyn IsMenuItem<tauri::Wry>)
        .collect();
    refs.push(&sep);
    refs.push(&settings_menu);
    refs.push(&about);
    refs.push(&quit);

    Menu::with_items(app, &refs)
}

// Update checkmarks in-place. Returns true if service list changed (requiring a full menu swap).
fn sync_items(
    items: &[(CheckMenuItem<tauri::Wry>, String)],
    services: &[(String, String)],
    active: Option<&str>,
) -> bool {
    let same = items.len() == services.len()
        && items.iter().zip(services).all(|((_, b), (_, sb))| b == sb);
    if same {
        for (item, bsd) in items {
            let _ = item.set_checked(active.as_deref() == Some(bsd.as_str()));
        }
    }
    !same
}

struct NetworkCtx {
    handle: AppHandle,
    items: SharedItems,
    previous_interface: Mutex<Option<String>>,
}

fn on_network_change(store: SCDynamicStore, _: CFArray<CFString>, ctx: &mut NetworkCtx) {
    let active = get_active_interface(&store);
    let services = get_network_services();
    let mut items = ctx.items.lock().unwrap();

    let prev_interface = ctx.previous_interface.lock().unwrap().clone();
    if active != prev_interface {
        let show_notifications = load_show_notifications(&ctx.handle);
        if show_notifications {
            if let Some(ref iface) = active {
                let display_name = get_display_name(iface);
                let _ = ctx
                    .handle
                    .notification()
                    .builder()
                    .title("Network Interface Changed")
                    .body(&format!("Switched to {}", display_name))
                    .show();
            }
        }
        *ctx.previous_interface.lock().unwrap() = active.clone();
    }

    let needs_rebuild = sync_items(&items, &services, active.as_deref());
    if needs_rebuild {
        let launch_at_login = is_launch_at_login_enabled();
        let show_notifications = load_show_notifications(&ctx.handle);
        if let Ok(new_items) = create_items(&ctx.handle, &services, active.as_deref()) {
            *items = new_items;
            if let Ok(menu) =
                assemble_menu(&ctx.handle, &items, launch_at_login, show_notifications)
            {
                if let Some(tray) = ctx.handle.tray_by_id("main") {
                    let _ = tray.set_menu(Some(menu));
                }
            }
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let init_store = SCDynamicStoreBuilder::new("net-init").build();
            let services = get_network_services();
            let active = get_active_interface(&init_store);
            let connected = check_internet();
            let icon = create_dot_icon(connected);

            let show_notifications = load_show_notifications(app.handle());
            if show_notifications {
                if let Some(ref iface) = active {
                    let display_name = get_display_name(iface);
                    let _ = app
                        .handle()
                        .notification()
                        .builder()
                        .title("Network Interface")
                        .body(&format!("Connected to {}", display_name))
                        .show();
                }
            }

            let items: SharedItems = Arc::new(Mutex::new(create_items(
                app.handle(),
                &services,
                active.as_deref(),
            )?));

            let launch_at_login = is_launch_at_login_enabled();
            let show_notifications = load_show_notifications(app.handle());
            let menu = assemble_menu(
                app.handle(),
                &items.lock().unwrap(),
                launch_at_login,
                show_notifications,
            )?;

            let items_for_event = items.clone();
            let app_handle = app.handle().clone();
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .on_menu_event(move |app: &AppHandle, event: MenuEvent| {
                    if event.id() == "quit" {
                        app.exit(0);
                    } else if event.id() == "about" {
                        let _ = std::process::Command::new("open")
                            .arg("https://github.com/comatory/network-device-status")
                            .spawn();
                    } else if event.id() == "launch_at_login" {
                        let current = is_launch_at_login_enabled();
                        let _ = toggle_launch_at_login(!current);
                    } else if event.id() == "show_notifications" {
                        let current = load_show_notifications(&app_handle);
                        let new_value = !current;
                        save_show_notifications(&app_handle, new_value);
                    } else {
                        // Open Network Settings
                        let _ = std::process::Command::new("open")
                            .arg("x-apple.systempreferences:com.apple.Network-Settings.extension")
                            .spawn();
                        // Reset CheckMenuItem toggle in-place (menu already closed on click)
                        let store = SCDynamicStoreBuilder::new("net-click").build();
                        let active = get_active_interface(&store);
                        let items = items_for_event.lock().unwrap();
                        for (item, bsd) in items.iter() {
                            let _ = item.set_checked(active.as_deref() == Some(bsd.as_str()));
                        }
                    }
                })
                .build(app)?;

            // Thread 1: SCDynamicStore event-driven interface monitoring
            let handle1 = app.handle().clone();
            let items_for_ctx = items.clone();
            let initial_interface = active.clone();
            thread::spawn(move || {
                let store = SCDynamicStoreBuilder::new("net-monitor")
                    .callback_context(SCDynamicStoreCallBackContext {
                        callout: on_network_change,
                        info: NetworkCtx {
                            handle: handle1,
                            items: items_for_ctx,
                            previous_interface: Mutex::new(initial_interface),
                        },
                    })
                    .build();

                let empty: CFArray<CFString> = CFArray::from_CFTypes(&[]);
                let patterns: CFArray<CFString> = CFArray::from_CFTypes(&[
                    CFString::new("State:/Network/Global/IPv4"),
                    CFString::new("State:/Network/Global/IPv6"),
                    CFString::new("State:/Network/Interface/.*"),
                ]);
                store.set_notification_keys(&empty, &patterns);

                let rl = CFRunLoop::get_current();
                let source = store.create_run_loop_source();
                unsafe {
                    rl.add_source(&source, kCFRunLoopDefaultMode);
                }
                CFRunLoop::run_current();
            });

            // Thread 2: connectivity poll (icon only)
            let handle2 = app.handle().clone();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(5));
                let connected = check_internet();
                if let Some(tray) = handle2.tray_by_id("main") {
                    let _ = tray.set_icon(Some(create_dot_icon(connected)));
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
