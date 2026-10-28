use anyhow::Result;
use macos_mcp_manager::{
    clients::Server,
    manager::{ClientView, Manager, Snapshot},
};
use objc2::{msg_send, rc::Retained, runtime::NSObjectProtocol, sel};
use objc2_app_kit::{NSImage, NSMenu, NSWorkspace};
use objc2_foundation::{NSCopying, NSSize, NSString};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    time::Duration,
};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::{ActivationPolicy, EventLoopExtMacOS},
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{
        CheckMenuItem, ContextMenu, IconMenuItem, Menu, MenuEvent, MenuId, MenuItem, NativeIcon,
        PredefinedMenuItem, Submenu,
    },
};

#[derive(Clone)]
enum Action {
    Toggle(Arc<ClientView>, Server),
    OpenConfig(PathBuf),
    Details(String),
    OpenUrl(&'static str),
    Quit,
}

enum UiEvent {
    Menu(MenuEvent),
    Updated(Result<Snapshot, String>),
    Failed(String),
}

pub fn run(manager: Manager) -> Result<()> {
    let icon = menu_bar_icon()?;
    let mut event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    event_loop.set_dock_visibility(false);
    event_loop.set_activate_ignoring_other_apps(false);
    let icons = MenuIcons::load()?;
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(UiEvent::Menu(event));
    }));
    let worker_proxy = event_loop.create_proxy();
    let (work_tx, work_rx) = mpsc::channel::<(Arc<ClientView>, Server)>();
    std::thread::spawn(move || {
        let mut previous: Option<Result<Snapshot, String>> = None;
        loop {
            let next = manager.snapshot().map_err(|error| error.to_string());
            if previous.as_ref() != Some(&next) {
                previous = Some(next.clone());
                if worker_proxy.send_event(UiEvent::Updated(next)).is_err() {
                    break;
                }
            }
            let (client, server) = match work_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if let Err(error) = manager.toggle(&client, &server) {
                let _ = worker_proxy
                    .send_event(UiEvent::Failed(format!("Change not applied: {error}")));
            }
            // Always send the actual state after a click, including a failed save, so the
            // native checkmark is reset if AppKit optimistically changed it.
            let next = manager.snapshot().map_err(|error| error.to_string());
            previous = Some(next.clone());
            if worker_proxy.send_event(UiEvent::Updated(next)).is_err() {
                break;
            }
        }
    });

    let mut tray: Option<TrayIcon> = None;
    let mut actions = HashMap::new();
    let mut epoch = 0_u64;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                let menu = Menu::new();
                let _ = menu.append(&IconMenuItem::with_native_icon(
                    "Loading MCP configurations…",
                    false,
                    Some(icons.get("hourglass")),
                    None,
                ));
                show_root_menu_icons(&menu);
                match TrayIconBuilder::new()
                    .with_icon_templated(icon.clone())
                    .with_tooltip("MCP Manager")
                    .with_menu(Box::new(menu))
                    .build()
                {
                    Ok(icon) => tray = Some(icon),
                    Err(_) => {
                        eprintln!("Cannot create the macOS menu bar item");
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::UserEvent(UiEvent::Updated(snapshot)) => {
                if let Some(tray) = &tray {
                    epoch += 1;
                    match build_menu(&snapshot, epoch, &icons) {
                        Ok((menu, next_actions)) => {
                            actions = next_actions;
                            tray.set_menu(Some(Box::new(menu)));
                        }
                        Err(_) => show_message(
                            "Could not rebuild the native menu. Quit and reopen MCP Manager.",
                        ),
                    }
                }
            }
            Event::UserEvent(UiEvent::Failed(message)) => show_message(&message),
            Event::UserEvent(UiEvent::Menu(event)) => {
                if let Some(action) = actions.get(&event.id).cloned() {
                    match action {
                        Action::Quit => *control_flow = ControlFlow::Exit,
                        Action::Toggle(client, server) => {
                            let _ = work_tx.send((client, server));
                        }
                        Action::OpenConfig(path) => open_configuration(&path),
                        Action::Details(message) => show_message(&message),
                        Action::OpenUrl(url) => {
                            let _ = Command::new("/usr/bin/open")
                                .arg(url)
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .spawn();
                        }
                    }
                }
            }
            _ => {}
        }
    });
}

fn build_menu(
    snapshot: &Result<Snapshot, String>,
    epoch: u64,
    icons: &MenuIcons,
) -> Result<(Menu, HashMap<MenuId, Action>)> {
    let menu = Menu::new();
    let mut actions = HashMap::new();
    let mut serial = 0;
    let mut id = || {
        serial += 1;
        MenuId::new(format!("{epoch}-{serial}"))
    };
    if let Ok(snapshot) = snapshot {
        for client in &snapshot.clients {
            let shared_client = Arc::new(client.clone());
            let submenu = Submenu::new(client.source.kind.name(), true);
            use macos_mcp_manager::clients::ClientKind;
            let symbol = match client.source.kind {
                ClientKind::Codex => "sparkles",
                ClientKind::ClaudeDesktop => "bubble.left.and.bubble.right",
                ClientKind::ClaudeCode => "terminal",
                ClientKind::VsCode => "curlybraces",
                ClientKind::Cursor => "cursorarrow",
            };
            submenu.set_native_icon(Some(icons.get(symbol)));
            if client.source.kind == macos_mcp_manager::clients::ClientKind::VsCode {
                submenu.append(&MenuItem::new(
                    "Checkmarks: configuration present",
                    false,
                    None,
                ))?;
            }
            if client.error.is_some() {
                submenu.append(&MenuItem::new("Configuration unavailable", false, None))?;
            } else if client.servers.is_empty() {
                submenu.append(&MenuItem::new("No user-configured servers", false, None))?;
            } else {
                for server in &client.servers {
                    let toggle = id();
                    submenu.append(&CheckMenuItem::with_id(
                        toggle.clone(),
                        menu_label(&server.name),
                        server.can_toggle,
                        server.enabled,
                        None,
                    ))?;
                    actions.insert(
                        toggle,
                        Action::Toggle(shared_client.clone(), server.clone()),
                    );
                }
            }
            submenu.append(&PredefinedMenuItem::separator())?;
            let guidance = id();
            submenu.append(&MenuItem::with_id(
                guidance.clone(),
                "Scope and reload instructions…",
                true,
                None,
            ))?;
            actions.insert(
                guidance,
                Action::Details(client.source.kind.guidance().into()),
            );
            if client.contents.is_some() {
                let open = id();
                submenu.append(&MenuItem::with_id(
                    open.clone(),
                    "Open Configuration…",
                    true,
                    None,
                ))?;
                actions.insert(open, Action::OpenConfig(client.source.path.clone()));
            }
            menu.append(&submenu)?;
        }
    } else {
        menu.append(&IconMenuItem::with_native_icon(
            "Configuration could not be loaded",
            false,
            Some(icons.get("exclamationmark.triangle")),
            None,
        ))?;
        if let Err(error) = snapshot {
            let details = id();
            menu.append(&IconMenuItem::with_id_and_native_icon(
                details.clone(),
                "Recovery details…",
                true,
                Some(icons.get("info.circle")),
                None,
            ))?;
            actions.insert(details, Action::Details(error.clone()));
        }
    }
    menu.append(&PredefinedMenuItem::separator())?;
    let repository = id();
    menu.append(&IconMenuItem::with_id_and_native_icon(
        repository.clone(),
        "GitHub repository",
        true,
        Some(icons.get("chevron.left.forwardslash.chevron.right")),
        None,
    ))?;
    actions.insert(
        repository,
        Action::OpenUrl("https://github.com/yurist38/macos-mcp-manager"),
    );
    let support = id();
    menu.append(&IconMenuItem::with_id_and_native_icon(
        support.clone(),
        "Support project",
        true,
        Some(icons.get("cup.and.saucer")),
        None,
    ))?;
    actions.insert(support, Action::OpenUrl("https://buymeacoffee.com/roomjs"));
    menu.append(&PredefinedMenuItem::separator())?;
    let quit = id();
    menu.append(&IconMenuItem::with_id_and_native_icon(
        quit.clone(),
        "Quit MCP Manager",
        true,
        Some(icons.get("power")),
        None,
    ))?;
    actions.insert(quit, Action::Quit);
    show_root_menu_icons(&menu);
    Ok((menu, actions))
}

fn show_root_menu_icons(menu: &Menu) {
    // This menu is owned by muda and accessed only on AppKit's main thread.
    let native_menu = unsafe { &*menu.ns_menu().cast::<NSMenu>() };
    for index in 0..native_menu.numberOfItems() {
        let Some(item) = native_menu.itemAtIndex(index) else {
            continue;
        };
        if item.image().is_some() && item.respondsToSelector(sel!(setPreferredImageVisibility:)) {
            // macOS 27 hides images in Automatic mode. The current objc2 bindings
            // predate this API; 1 is NSMenuItemImageVisibilityVisible (NSInteger).
            // Checking the selector keeps this compatible with macOS 14–26.
            unsafe {
                let _: () = msg_send![&*item, setPreferredImageVisibility: 1_isize];
            }
        }
    }
}

// Keep the named image assets alive alongside the event loop. The menu library
// retains the native icon names in its item state, including cloned/rebuilt menus.
struct MenuIcons(BTreeMap<&'static str, Retained<NSImage>>);

impl MenuIcons {
    fn load() -> Result<Self> {
        let mut images = BTreeMap::new();
        for symbol in [
            "sparkles",
            "bubble.left.and.bubble.right",
            "terminal",
            "curlybraces",
            "cursorarrow",
            "chevron.left.forwardslash.chevron.right",
            "cup.and.saucer",
            "power",
            "hourglass",
            "exclamationmark.triangle",
            "info.circle",
        ] {
            let application_icon = Self::installed_client_icon(symbol);
            let is_application_icon = application_icon.is_some();
            let image = application_icon
                .or_else(|| {
                    NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str(symbol),
                        None,
                    )
                })
                .ok_or_else(|| anyhow::anyhow!("Cannot load menu icon: {symbol}"))?
                .copy();
            let size = image.size();
            anyhow::ensure!(
                size.width > 0.0 && size.height > 0.0,
                "Menu icon has no size: {symbol}"
            );
            let scale = 16.0 / size.width.max(size.height);
            image.setSize(NSSize::new(size.width * scale, size.height * scale));
            image.setTemplate(!is_application_icon);
            anyhow::ensure!(
                image.setName(Some(&NSString::from_str(&Self::name(symbol)))),
                "Cannot register menu icon: {symbol}"
            );
            images.insert(symbol, image);
        }
        Ok(Self(images))
    }

    fn installed_client_icon(symbol: &str) -> Option<Retained<NSImage>> {
        let bundles: &[&str] = match symbol {
            "sparkles" => &["com.openai.codex", "com.openai.chat"],
            "bubble.left.and.bubble.right" | "terminal" => &["com.anthropic.claudefordesktop"],
            "curlybraces" => &["com.microsoft.VSCode", "com.microsoft.VSCodeInsiders"],
            "cursorarrow" => &["com.todesktop.230313mzl4w4u92"],
            _ => return None,
        };
        let workspace = NSWorkspace::sharedWorkspace();
        bundles.iter().find_map(|bundle| {
            let url =
                workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle))?;
            let path = url.path()?;
            Some(workspace.iconForFile(&path))
        })
    }

    fn name(symbol: &str) -> String {
        format!("dev.mcp-manager.menu.{symbol}")
    }

    fn get(&self, symbol: &'static str) -> NativeIcon {
        assert!(self.0.contains_key(symbol), "unregistered menu icon");
        NativeIcon::from_name(Self::name(symbol))
    }
}

fn menu_label(value: &str) -> String {
    let mut result: String = value.chars().filter(|c| !c.is_control()).take(90).collect();
    if value.chars().count() > 90 {
        result.push('…');
    }
    // muda uses ampersands for mnemonics; double them to display server names literally.
    result.replace('&', "&&")
}

fn menu_bar_icon() -> Result<Icon> {
    // Draw at 2× resolution. macOS uses the alpha channel to recolor the template.
    const SIZE: u32 = 44;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - 22.0;
            let dy = y as f32 + 0.5 - 22.0;
            let circle = (dx.hypot(dy) - 17.0).abs() - 1.5;
            let lines = [-6.0_f32, 0.0, 6.0]
                .into_iter()
                .map(|offset| (dx.abs() - 7.0).max(0.0).hypot(dy - offset) - 1.5)
                .fold(f32::INFINITY, f32::min);
            let alpha = ((0.5 - circle.min(lines)).clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}

fn open_configuration(path: &Path) {
    let mut command = Command::new("/usr/bin/open");
    command.arg("-t");
    let _ = command
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub(crate) fn show_message(message: &str) {
    // User-controlled names are arguments, never interpolated into AppleScript source.
    let script = "on run argv\ndisplay alert \"MCP Manager\" message (item 1 of argv) buttons {\"OK\"} default button \"OK\"\nend run";
    let _ = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .arg(message)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
#[allow(dead_code)] // Invoked by the separate main-thread integration test.
pub(crate) fn verify_menu_icons() {
    use objc2_app_kit::NSMenu;
    use tray_icon::menu::ContextMenu;
    let _event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let icons = MenuIcons::load().unwrap();
    let clients = macos_mcp_manager::clients::sources(std::path::Path::new("/fixture"), None)
        .into_iter()
        .map(|source| ClientView {
            source,
            servers: Vec::new(),
            error: None,
            contents: None,
        })
        .collect();
    let (menu, _) = build_menu(&Ok(Snapshot { clients }), 1, &icons).unwrap();
    let rebuilt = Menu::new();
    for item in menu.items() {
        use tray_icon::menu::MenuItemKind;
        match item {
            MenuItemKind::Submenu(item) => rebuilt.append(&item).unwrap(),
            MenuItemKind::Icon(item) => rebuilt.append(&item).unwrap(),
            MenuItemKind::Predefined(item) => rebuilt.append(&item).unwrap(),
            MenuItemKind::MenuItem(item) => rebuilt.append(&item).unwrap(),
            MenuItemKind::Check(item) => rebuilt.append(&item).unwrap(),
        }
    }
    fn assert_visible(menu: &Menu) {
        let native = unsafe { &*menu.ns_menu().cast::<NSMenu>() };
        let mut count = 0;
        for index in 0..native.numberOfItems() {
            let item = native.itemAtIndex(index).unwrap();
            if item.isSeparatorItem() {
                continue;
            }
            let image = item.image().expect("missing native menu image");
            assert!(image.size().width > 0.0 && image.size().height > 0.0);
            if matches!(
                item.title().to_string().as_str(),
                "GitHub repository" | "Support project" | "Quit MCP Manager"
            ) {
                assert!(image.isTemplate());
            }
            if item.respondsToSelector(sel!(preferredImageVisibility)) {
                let visibility: isize = unsafe { msg_send![&*item, preferredImageVisibility] };
                assert_eq!(
                    visibility, 1,
                    "menu image visibility must be Visible, not Automatic"
                );
            }
            count += 1;
        }
        assert_eq!(count, 8);
    }
    // Check the production builder, not just image presence on a copied menu.
    assert_visible(&menu);
    show_root_menu_icons(&rebuilt);
    assert_visible(&rebuilt);
    println!("Verified all 8 root icons have images and explicit visibility on macOS 27.");
}
