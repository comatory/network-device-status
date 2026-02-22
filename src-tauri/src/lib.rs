use std::{
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle,
};
use system_configuration::{
    core_foundation::{
        array::CFArray,
        base::TCFType,
        runloop::{CFRunLoop, kCFRunLoopDefaultMode},
        string::CFString,
    },
    dynamic_store::{SCDynamicStore, SCDynamicStoreBuilder, SCDynamicStoreCallBackContext},
    network_configuration::SCNetworkService,
    preferences::SCPreferences,
};

// (CheckMenuItem, bsd_name)
type SharedItems = Arc<Mutex<Vec<(CheckMenuItem<tauri::Wry>, String)>>>;

fn check_internet() -> bool {
    TcpStream::connect_timeout(
        &"1.1.1.1:53".parse::<std::net::SocketAddr>().unwrap(),
        Duration::from_secs(3),
    )
    .is_ok()
}

fn get_active_interface(store: &SCDynamicStore) -> Option<String> {
    use system_configuration::core_foundation::{
        base::CFType,
        dictionary::CFDictionary,
    };

    let key = CFString::new("State:/Network/Global/IPv4");
    let plist = store.get(key)?;

    let dict: CFDictionary<CFString, CFType> = unsafe {
        CFDictionary::wrap_under_get_rule(plist.as_concrete_TypeRef() as _)
    };

    let iface_key = CFString::new("PrimaryInterface");
    let value = dict.find(iface_key)?;
    value.downcast::<CFString>().map(|s: CFString| s.to_string())
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
) -> tauri::Result<Menu<tauri::Wry>> {
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let mut refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        items.iter().map(|(i, _)| i as &dyn IsMenuItem<tauri::Wry>).collect();
    refs.push(&sep);
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
}

fn on_network_change(store: SCDynamicStore, _: CFArray<CFString>, ctx: &mut NetworkCtx) {
    let active = get_active_interface(&store);
    let services = get_network_services();
    let mut items = ctx.items.lock().unwrap();

    let needs_rebuild = sync_items(&items, &services, active.as_deref());
    if needs_rebuild {
        // Services list changed — replace menu (will close if open, but this is rare)
        if let Ok(new_items) = create_items(&ctx.handle, &services, active.as_deref()) {
            *items = new_items;
            if let Ok(menu) = assemble_menu(&ctx.handle, &items) {
                if let Some(tray) = ctx.handle.tray_by_id("main") {
                    let _ = tray.set_menu(Some(menu));
                }
            }
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let init_store = SCDynamicStoreBuilder::new("net-init").build();
            let services = get_network_services();
            let active = get_active_interface(&init_store);
            let connected = check_internet();
            let icon = create_dot_icon(connected);

            let items: SharedItems = Arc::new(Mutex::new(
                create_items(app.handle(), &services, active.as_deref())?,
            ));

            let menu = assemble_menu(app.handle(), &items.lock().unwrap())?;

            let items_for_event = items.clone();
            let _tray = TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&menu)
                .on_menu_event(move |app: &AppHandle, event: MenuEvent| {
                    if event.id() == "quit" {
                        app.exit(0);
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
            thread::spawn(move || {
                let store = SCDynamicStoreBuilder::new("net-monitor")
                    .callback_context(SCDynamicStoreCallBackContext {
                        callout: on_network_change,
                        info: NetworkCtx { handle: handle1, items: items_for_ctx },
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
