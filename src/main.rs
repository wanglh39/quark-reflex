#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod cred;
mod icon;
mod settings;
mod titlebar;

use config::{Config, BAR_H};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use serde_json::Value;
use std::time::Duration;
use tao::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Icon, Window, WindowBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::w;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, ERROR_ALREADY_EXISTS, WAIT_OBJECT_0,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
use wry::{Rect, WebContext, WebView, WebViewBuilder, NewWindowResponse, WebViewBuilderExtWindows};

#[derive(Debug)]
enum AppEvent {
    Wake,
    HotkeyPressed(u32),
    TrayMenu(String),
    IpcBar(String),
    IpcSettings(String),
    CredsReported(bool, String),
    Probe(u32),
    AgentMsg(String),
}

struct SingleGuard(#[allow(dead_code)] HANDLE);

fn inject_cookies(chat: &WebView) {
    let txt = match std::fs::read_to_string(cred::cred_path()) {
        Ok(t) => t,
        Err(_) => return,
    };
    let cv: Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(_) => return,
    };
    let list = match cv.get("cookies").and_then(|c| c.as_array()) {
        Some(l) => l.clone(),
        None => return,
    };
    let mut ok_n = 0u32;
    for c in &list {
        let (Some(name), Some(value)) = (
            c.get("name").and_then(|x| x.as_str()),
            c.get("value").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        let mut b = wry::cookie::Cookie::build((name.to_string(), value.to_string()));
        if let Some(d) = c.get("domain").and_then(|x| x.as_str()) {
            b = b.domain(d.to_string());
        }
        if let Some(p) = c.get("path").and_then(|x| x.as_str()) {
            b = b.path(p.to_string());
        }
        if c.get("httpOnly").and_then(|x| x.as_bool()) == Some(true) {
            b = b.http_only(true);
        }
        if c.get("secure").and_then(|x| x.as_bool()) == Some(true) {
            b = b.secure(true);
        }
        match c.get("sameSite").and_then(|x| x.as_str()) {
            Some("Strict") => b = b.same_site(wry::cookie::SameSite::Strict),
            Some("Lax") => b = b.same_site(wry::cookie::SameSite::Lax),
            Some("None") => b = b.same_site(wry::cookie::SameSite::None),
            _ => {}
        }
        if let Some(exp) = c.get("expires").and_then(|x| x.as_f64()) {
            if exp > 0.0 {
                if let Some(dt) = wry::cookie::time::OffsetDateTime::from_unix_timestamp(exp as i64)
                    .ok()
                {
                    b = b.expires(dt);
                }
            }
        }
        if chat.set_cookie(&b.build()).is_ok() {
            ok_n += 1;
        }
    }
    let _ = std::fs::write(config::app_dir().join("cookie-inject.log"), format!("{ok_n}/{}", list.len()));
}

fn webview2_runtime_missing() -> bool {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
    const RUNTIME_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9FC7E51}";
    for root in [LOCAL_MACHINE, CURRENT_USER] {
        for sub in [
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{RUNTIME_GUID}"),
            format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{RUNTIME_GUID}"),
        ] {
            if let Ok(key) = root.open(&sub) {
                if let Ok(pv) = key.get_string("pv") {
                    if !pv.trim().is_empty() {
                        return false;
                    }
                }
            }
        }
    }
    for base in [
        r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
        r"C:\Program Files\Microsoft\EdgeWebView\Application",
    ] {
        if let Ok(entries) = std::fs::read_dir(base) {
            if entries.filter_map(|e| e.ok()).any(|e| e.path().join("msedgewebview2.exe").is_file()) {
                return false;
            }
        }
    }
    for p in [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ] {
        if std::path::Path::new(p).is_file() {
            return false;
        }
    }
    if let Ok(dir) = std::env::var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER") {
        if !dir.trim().is_empty() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn webview2_detected_on_local_machine() {
        assert!(
            !super::webview2_runtime_missing(),
            "本机装有 WebView2（EdgeWebView 155.0.4283.62），预检必须放行，不能误报"
        );
    }
}

fn confirm_start_without_webview2() -> bool {    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONWARNING, MB_YESNO};
    let r = unsafe {
        MessageBoxW(
            None,
            windows::core::w!(
                "未检测到 WebView2 运行时，小窗可能无法启动。\n\n\
                 建议从微软官网安装「WebView2 Runtime」：\n\
                 https://developer.microsoft.com/microsoft-edge/webview2/\n\n\
                 是否仍要尝试启动？"
            ),
            w!("夸克助手"),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    r == IDYES
}

fn fatal_msg(text: &str) {
    unsafe {
        MessageBoxW(
            None,
            &windows::core::HSTRING::from(text),
            w!("夸克助手"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn single_instance_acquire() -> Option<SingleGuard> {
    unsafe {
        let mutex = CreateMutexW(None, false, w!("Local\\QuarkReflex.SingleInstance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(ev) = CreateEventW(None, false, false, w!("Local\\QuarkReflex.Wake")) {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
            let _ = CloseHandle(mutex);
            return None;
        }
        Some(SingleGuard(mutex))
    }
}

fn spawn_wake_listener(proxy: EventLoopProxy<AppEvent>) {
    unsafe {
        let Ok(ev) = CreateEventW(None, false, false, w!("Local\\QuarkReflex.Wake")) else {
            return;
        };
        let ev_raw = ev.0 as isize;

        std::thread::spawn(move || {
            let ev = HANDLE(ev_raw as *mut core::ffi::c_void);
            loop {
                let r = WaitForSingleObject(ev, u32::MAX);
                if r == WAIT_OBJECT_0 {
                    let _ = proxy.send_event(AppEvent::Wake);
                }
            }
        });
    }
}

fn poll_channels(proxy: EventLoopProxy<AppEvent>) {
    std::thread::spawn(move || loop {
        while let Ok(e) = GlobalHotKeyEvent::receiver().try_recv() {
            if e.state() == HotKeyState::Pressed {
                let _ = proxy.send_event(AppEvent::HotkeyPressed(e.id()));
            }
        }
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                let _ = proxy.send_event(AppEvent::Wake);
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            let _ = proxy.send_event(AppEvent::TrayMenu(e.id().0.clone()));
        }
        std::thread::sleep(Duration::from_millis(25));
    });
}

fn apply_round_corners(window: &Window) {
    unsafe {
        let pref: i32 = DWMWCP_ROUND.0;
        let _ = DwmSetWindowAttribute(
            windows::Win32::Foundation::HWND(window.hwnd() as _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

fn center_window(window: &Window) {
    let Some(mon) = window
        .primary_monitor()
        .or_else(|| window.available_monitors().next())
    else {
        return;
    };
    let mp = mon.position();
    let ms = mon.size();
    let ws = window.outer_size();
    let x = mp.x + (ms.width as i32 - ws.width as i32).max(0) / 2;
    let y = mp.y + (ms.height as i32 - ws.height as i32).max(0) / 2;
    window.set_outer_position(PhysicalPosition::new(x, y));
}

fn ensure_on_screen(window: &Window) {
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let monitors: Vec<_> = window.available_monitors().collect();
    let on_screen = monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        pos.x >= mp.x
            && pos.x < mp.x + ms.width as i32
            && pos.y >= mp.y
            && pos.y < mp.y + ms.height as i32
    });
    if !on_screen {
        center_window(window);
    }
}

fn snapshot(window: &Window, config: &mut Config) {
    if let Ok(p) = window.outer_position() {
        let s = window.inner_size();
        config.window = Some(config::WinGeom {
            x: p.x,
            y: p.y,
            w: s.width as f64,
            h: s.height as f64,
        });
    }
}

fn show_main(
    window: &Window,
    chat: &WebView,
    settings_ui: &Option<(Window, WebView)>,
    settings_open: bool,
) {
    ensure_on_screen(window);
    if window.is_minimized() {
        window.set_minimized(false);
    }
    if !window.is_visible() {
        window.set_visible(true);
    }
    if settings_open {
        if let Some((w, _)) = settings_ui {
            w.set_visible(true);
        }
    }
    window.set_focus();
    let _ = chat.focus();
}

fn collapse_main(
    window: &Window,
    config: &mut Config,
    settings_ui: &Option<(Window, WebView)>,
    settings_open: &mut bool,
) {
    *settings_open = settings_ui
        .as_ref()
        .map(|(w, _)| w.is_visible())
        .unwrap_or(false);
    snapshot(window, config);
    config::save(config);
    if config.collapse_mode == "minimize" {
        window.set_minimized(true);
    } else {
        window.set_visible(false);
    }
}

fn toggle_main(
    window: &Window,
    config: &mut Config,
    chat: &WebView,
    settings_ui: &Option<(Window, WebView)>,
    settings_open: &mut bool,
) {
    let settings_visible = settings_ui
        .as_ref()
        .map(|(w, _)| w.is_visible())
        .unwrap_or(false);
    if settings_visible {
        if let Some((w, _)) = settings_ui {
            w.set_focus();
        }
        return;
    }
    if window.is_minimized() || !window.is_visible() {
        show_main(window, chat, settings_ui, *settings_open);
    } else if window.is_focused() {
        collapse_main(window, config, settings_ui, settings_open);
    } else {
        window.set_focus();
    }
}

fn layout(window: &Window, bar: &WebView, chat: &WebView) {
    let scale = window.scale_factor();
    let s = window.inner_size().to_logical::<f64>(scale);
    let _ = bar.set_bounds(Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(s.width.max(1.0), BAR_H).into(),
    });
    let _ = chat.set_bounds(Rect {
        position: LogicalPosition::new(0.0, BAR_H).into(),
        size: LogicalSize::new(s.width.max(1.0), (s.height - BAR_H).max(1.0)).into(),
    });
}

fn build_tray(rgba: &[u8], w: u32, h: u32) -> tray_icon::Result<TrayIcon> {
    let icon =
        tray_icon::Icon::from_rgba(rgba.to_vec(), w, h).expect("生成托盘图标失败");
    let menu = Menu::with_id("tray");
    let show = MenuItem::with_id("show", "显示小窗", true, None);
    let grab_creds = MenuItem::with_id("grab_creds", "获取登录凭证", true, None);
    let settings = MenuItem::with_id("settings", "设置...", true, None);
    let devtools = MenuItem::with_id("devtools", "开发者工具", true, None);
    let quit = MenuItem::with_id("exit", "退出", true, None);
    let _ = menu.append(&show);
    let _ = menu.append(&grab_creds);
    let _ = menu.append(&settings);
    let _ = menu.append(&devtools);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("夸克助手")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
}

fn center_over_owner(w: &Window, owner: &Window) {
    let Ok(op) = owner.outer_position() else {
        return;
    };
    let os = owner.outer_size();
    let ws = w.outer_size();
    let mut x = op.x + (os.width as i32 - ws.width as i32) / 2;
    let mut y = op.y + (os.height as i32 - ws.height as i32) / 2;
    if let Some(mon) = owner.current_monitor().or_else(|| owner.primary_monitor()) {
        let mp = mon.position();
        let ms = mon.size();
        x = x.clamp(mp.x, (mp.x + ms.width as i32 - ws.width as i32).max(mp.x));
        y = y.clamp(mp.y, (mp.y + ms.height as i32 - ws.height as i32).max(mp.y));
    }
    w.set_outer_position(PhysicalPosition::new(x, y));
}

fn open_settings(
    elw: &EventLoopWindowTarget<AppEvent>,
    web_ctx: &mut WebContext,
    proxy: &EventLoopProxy<AppEvent>,
    config: &Config,
    settings_ui: &mut Option<(Window, WebView)>,
    win_icon: &Option<Icon>,
    main_window: &Window,
    hotkey: &HotKey,
    hotkey_manager: &GlobalHotKeyManager,
) {
    if let Some((w, v)) = settings_ui {
        let _ = v.load_html(&settings::render(config));
        center_over_owner(w, main_window);
        w.set_visible(true);
        w.set_focus();
        return;
    }
    main_window.set_enable(false);
    let _ = hotkey_manager.unregister(*hotkey);
    let mut wb = WindowBuilder::new()
        .with_title("设置 - 夸克助手")
        .with_decorations(false)
        .with_resizable(false)
        .with_owner_window(main_window.hwnd())
        .with_skip_taskbar(true)
        .with_undecorated_shadow(true)
        .with_inner_size(LogicalSize::new(440.0, 718.0));
    if let Some(ic) = &win_icon {
        wb = wb.with_window_icon(Some(ic.clone()));
    }
    let w = wb.build(elw).expect("创建设置窗口失败");
    apply_round_corners(&w);
    let v = WebViewBuilder::new_with_web_context(web_ctx)
        .with_html(settings::render(config))
        .with_ipc_handler({
            let p = proxy.clone();
            move |req| {
                let _ = p.send_event(AppEvent::IpcSettings(req.body().clone()));
            }
        })
        .build_as_child(&w)
        .expect("创建设置视图失败");
    let scale = w.scale_factor();
    let s = w.inner_size().to_logical::<f64>(scale);
    let _ = v.set_bounds(Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(s.width, s.height).into(),
    });
    center_over_owner(&w, main_window);
    w.set_focus();
    *settings_ui = Some((w, v));
}

fn resume_from_settings(window: &Window, hotkey: &HotKey, manager: &GlobalHotKeyManager) {
    window.set_enable(true);
    let _ = manager.unregister(*hotkey);
    if manager.register(*hotkey).is_err() {
        let msg = format!("热键 {} 恢复注册失败，可能已被其他程序占用。", hotkey);
        unsafe {
            MessageBoxW(
                None,
                &windows::core::HSTRING::from(&msg),
                w!("夸克助手"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
}

fn clamp_size(w: f64, h: f64) -> config::Size {
    config::Size {
        w: w.clamp(320.0, 2000.0),
        h: h.clamp(420.0, 2000.0),
    }
}

fn apply_settings(
    v: &Value,
    config: &mut Config,
    hotkey: &mut HotKey,
    manager: &GlobalHotKeyManager,
    window: &Window,
    chat: &WebView,
) -> (bool, String) {
    if let Some(hk) = v.get("hotkey").and_then(|x| x.as_str()) {
        if hk != config.hotkey {
            let parsed: Result<HotKey, _> = hk.parse();
            match parsed {
                Err(_) => return (false, "热键格式无效".into()),
                Ok(new_hk) => {
                    if manager.register(new_hk).is_err() {
                        return (false, "注册失败：组合键可能已被其他程序占用".into());
                    }
                    let _ = manager.unregister(*hotkey);
                    *hotkey = new_hk;
                    config.hotkey = hk.to_string();
                }
            }
        }
    }
    if let Some(m) = v.get("collapse").and_then(|x| x.as_str()) {
        if m == "hide" || m == "minimize" {
            config.collapse_mode = m.into();
        }
    }
    let mut need_resize = false;
    if let (Some(w), Some(h)) = (
        v.get("cw").and_then(|x| x.as_f64()),
        v.get("ch").and_then(|x| x.as_f64()),
    ) {
        config.compact = clamp_size(w, h);
        if !config.reading_mode {
            need_resize = true;
        }
    }
    if let (Some(w), Some(h)) = (
        v.get("rw").and_then(|x| x.as_f64()),
        v.get("rh").and_then(|x| x.as_f64()),
    ) {
        config.reading = clamp_size(w, h);
        if config.reading_mode {
            need_resize = true;
        }
    }
    if need_resize {
        let sz = if config.reading_mode {
            config.reading
        } else {
            config.compact
        };
        window.set_inner_size(LogicalSize::new(sz.w, sz.h + BAR_H));
    }
    if let Some(z) = v.get("zoom").and_then(|x| x.as_f64()) {
        if (0.4..=2.0).contains(&z) {
            config.zoom = z;
            let _ = chat.zoom(z);
        }
    }
    if let Some(p) = v.get("qpath").and_then(|x| x.as_str()) {
        config.quark_exe = p.trim().to_string();
    }
    snapshot(window, config);
    config::save(config);
    (true, "已保存".into())
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::create_dir_all(config::app_dir());
        let _ = std::fs::write(config::app_dir().join("panic.log"), format!("{info}"));
    }));
    let _single = match single_instance_acquire() {
        Some(g) => g,
        None => {
            unsafe {
                MessageBoxW(
                    None,
                    w!("夸克助手已在运行，已为你唤起已有窗口。"),
                    w!("夸克助手"),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            return;
        }
    };

    let mut config = config::load();

    if webview2_runtime_missing() && !confirm_start_without_webview2() {
        return;
    }

    let event_loop = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    spawn_wake_listener(proxy.clone());
    poll_channels(proxy.clone());


    let (rgba, iw, ih) = icon::app_icon_rgba();
    let win_icon = Icon::from_rgba(rgba.clone(), iw, ih).ok();

    let size = if config.reading_mode {
        config.reading
    } else {
        config.compact
    };
    let mut wb = WindowBuilder::new()
        .with_title("夸克助手")
        .with_decorations(false)
        .with_resizable(true)
        .with_min_inner_size(LogicalSize::new(320.0, 420.0))
        .with_inner_size(LogicalSize::new(size.w, size.h + BAR_H));
    if let Some(ic) = win_icon.clone() {
        wb = wb.with_window_icon(Some(ic));
    }
    if let Some(g) = config.window {
        wb = wb.with_position(PhysicalPosition::new(g.x, g.y));
    }
    let window = wb.build(&event_loop).expect("创建主窗口失败");
    apply_round_corners(&window);
    if config.window.is_none() {
        center_window(&window);
    }

    let mut web_ctx = WebContext::new(Some(config::webview_data_dir()));

    let bar = match WebViewBuilder::new_with_web_context(&mut web_ctx)
        .with_html(titlebar::HTML)
        .with_ipc_handler({
            let p = proxy.clone();
            move |req| {
                let _ = p.send_event(AppEvent::IpcBar(req.body().clone()));
            }
        })
        .build_as_child(&window)
    {
        Ok(v) => v,
        Err(e) => {
            fatal_msg(&format!("创建标题栏失败：{e}"));
            return;
        }
    };

    let cred_key = cred::gen_key();
    let chat = match WebViewBuilder::new_with_web_context(&mut web_ctx)
        .with_url(config::CHAT_URL)
        .with_user_agent(config::UA)
        .with_https_scheme(true)
        .with_custom_protocol(cred::PROTOCOL_NAME.into(), {
            let key = cred_key.clone();
            move |_id, req| cred::handle_qrcred(&key, req)
        })
        .with_initialization_script(cred::build_inject_js(&cred_key) + config::LOGIN_FLOW_JS)
        .with_devtools(true)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .build_as_child(&window)
    {
        Ok(v) => v,
        Err(e) => {
            fatal_msg(&format!("创建聊天视图失败：{e}\n\n若反复出现，请重装 WebView2 Runtime 后重试。"));
            return;
        }
    };
    let _ = chat.zoom(config.zoom);

    layout(&window, &bar, &chat);

    let hotkey_manager = GlobalHotKeyManager::new().expect("热键管理器初始化失败");
    let mut hotkey: HotKey = config.hotkey.parse().expect("配置的热键无效");
    if hotkey_manager.register(hotkey).is_err() {
        let msg = format!(
            "热键 {} 注册失败，可能已被其他程序占用，可在设置中更换。",
            config.hotkey
        );
        unsafe {
                MessageBoxW(
                    None,
                    &windows::core::HSTRING::from(&msg),
                    w!("夸克助手"),
                    MB_OK | MB_ICONINFORMATION,
                );
        }
    }

    let _tray = build_tray(&rgba, iw, ih).expect("托盘创建失败");

    cred::start_grab(proxy.clone());

    if config.pin {
        window.set_always_on_top(true);
    }

    let mut settings_ui: Option<(Window, WebView)> = None;
    let mut settings_open = false;
    let main_id = window.id();

    event_loop.run(move |event, elw, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(AppEvent::Wake) => {
                show_main(&window, &chat, &settings_ui, settings_open);
            }
            Event::UserEvent(AppEvent::HotkeyPressed(id)) => {
                if id == hotkey.id() {
                    toggle_main(&window, &mut config, &chat, &settings_ui, &mut settings_open);
                }
            }
            Event::UserEvent(AppEvent::TrayMenu(mid)) => match mid.as_str() {
                "show" => show_main(&window, &chat, &settings_ui, settings_open),
                "grab_creds" => {
                    cred::start_grab(proxy.clone());
                }
                "settings" => {
                    open_settings(
                        elw,
                        &mut web_ctx,
                        &proxy,
                        &config,
                        &mut settings_ui,
                        &win_icon,
                        &window,
                        &hotkey,
                        &hotkey_manager,
                    )
                }
                "devtools" => {
                    chat.open_devtools();
                }
                "exit" => {
                    snapshot(&window, &mut config);
                    config::save(&config);
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            Event::UserEvent(AppEvent::CredsReported(ok, msg)) => {
                if ok {
                    show_main(&window, &chat, &settings_ui, settings_open);
                    inject_cookies(&chat);
                    cred::start_agent_poll();
                    let _ = chat.load_url(config::CHAT_URL);
                    if std::env::var("QR_PROBE").as_deref() == Ok("1") {
                        for n in [1u32, 2, 3, 4, 5] {
                            let p = proxy.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_secs(10 * n as u64));
                                let _ = p.send_event(AppEvent::Probe(n));
                            });
                        }
                    }
                } else {
                    unsafe {
                        MessageBoxW(
                            None,
                            &windows::core::HSTRING::from(&msg),
                            w!("夸克助手"),
                            MB_OK | MB_ICONINFORMATION,
                        );
                    }
                }
            }
            Event::UserEvent(AppEvent::Probe(n)) => {
                let js = r#"(function(){try{
                    var c=window.__qrCreds;
                    var up=c&&(c.ucparams||c.public_params);
                    var cont=document.querySelector('#ice-container')||document.querySelector('#root');
                    var nl='';try{nl=sessionStorage.getItem('__qrNavLog')||''}catch(e){}
                    var al='';try{al=sessionStorage.getItem('__qrApiLog')||''}catch(e){}
                    var el='';try{el=sessionStorage.getItem('__qrErrLog')||''}catch(e){}
                    var xl='';try{xl=sessionStorage.getItem('__qrXLog')||''}catch(e){}
                    var ml='';try{ml=sessionStorage.getItem('__qrMissLog')||''}catch(e){}
                    var nav=(performance.getEntriesByType('navigation')||[])[0]||{};
                    return JSON.stringify({
                    probe:1,
                    href:location.href.slice(0,80),
                    ref:document.referrer.slice(0,60),
                    navLog:nl.slice(-500),
                    apiLog:al.slice(-800),
                    errLog:el.slice(-2400),
                    xLog:xl.slice(-1400),
                    missLog:ml.slice(-2000),
                    navStatus:nav.responseStatus,
                    navType:nav.type,
                    domHead:(document.body?document.body.outerHTML:'NOBODY').replace(/\s+/g,' ').slice(0,500),
                    bridge:window.__qrBridgeState||null,
                    hasUcapi:!!(window.ucapi&&window.ucapi.invoke),
                    hasAccount:!!(window.chrome&&window.chrome.account&&window.chrome.account.getUserInfo),
                    hasSDK:!!window.__QIANWEN_CHAT_SDK__,
                    rendered:!!(cont&&cont.children.length>0),
                    reactRoot:(function(){var c=document.getElementById('ice-container');if(!c)return 'nocont';var ks=Object.keys(c).filter(function(k){return k.indexOf('__react')===0||k.indexOf('_react')===0});return ks.length?ks.join(','):'bare'})(),
                    scripts:(function(){return Array.prototype.slice.call(document.scripts||[]).slice(0,30).map(function(sc){var u=sc.src?('S:'+sc.src):('I:'+String(sc.textContent||'').slice(0,12).replace(/\s+/g,''));return u.replace(/^https?:\/\/[^/]+\//,'').slice(0,60)+(sc.async?'|a':'')+(sc.defer?'|d':'')})})(),
                    mainInit:(function(){var e=(performance.getEntriesByType('resource')||[]).filter(function(x){return x.name.indexOf('/main.js')>=0})[0];return e?String(e.initiatorType):'none'})(),
                    bodyHead:(document.body.innerText||'').replace(/\s+/g,' ').slice(0,200),
                    api:(performance.getEntriesByType('resource')||[]).filter(function(e){return e.name.indexOf('chat-api-quark2')>=0}).length,
                    apiList:(performance.getEntriesByType('resource')||[]).filter(function(e){return /chat-api|qianwen\.com\/api|aide\.qianwen|quark\.cn\/api/.test(e.name)}).map(function(e){return e.name.replace(/^https?:\/\//,'').split('?')[0].slice(0,80)}),
                    chunkList:(function(){var m={};(performance.getEntriesByType('resource')||[]).forEach(function(e){var i=e.name.indexOf('alicdn.com');if(i>=0){var f=e.name.slice(i).split('?')[0];if(/\.js$|\.css$/.test(f))m[f]=1}});return Object.keys(m)})(),
                    chromeCalls:(window.__qrChromeLog||[]).slice(-40),
                    chromeCount:(window.__qrChromeLog||[]).length,
                    ucCalls2:(window.__qrUcLog||[]).slice(-20)
                    })
                }catch(e){return 'EVALERR:'+e.message}})()"#;
                let tag = n;
                let r = chat.evaluate_script_with_callback(js, move |s| {
                    let _ = std::fs::create_dir_all(config::app_dir());
                    let _ = std::fs::write(config::app_dir().join("chat-verify.log"), s);
                    let _ = tag;
                });
                if r.is_err() {
                    let _ = std::fs::write(config::app_dir().join("chat-verify.log"), "EVAL_FAIL");
                }
            }
            Event::UserEvent(AppEvent::AgentMsg(_s)) => {}
            Event::UserEvent(AppEvent::IpcBar(s)) => {
                let Ok(v) = serde_json::from_str::<Value>(&s) else {
                    return;
                };
                match v.get("cmd").and_then(|c| c.as_str()) {
                    Some("drag") => {
                        let _ = window.drag_window();
                    }
                    Some("pin") => {
                        config.pin = !config.pin;
                        window.set_always_on_top(config.pin);
                        let _ =
                            bar.evaluate_script(&format!("window.__qrSetPin({})", config.pin));
                    }
                    Some("toggle_size") => {
                        config.reading_mode = !config.reading_mode;
                        let sz = if config.reading_mode {
                            config.reading
                        } else {
                            config.compact
                        };
                        window.set_inner_size(LogicalSize::new(sz.w, sz.h + BAR_H));
                    }
                    Some("settings") => {
                        open_settings(
                            elw,
                            &mut web_ctx,
                            &proxy,
                            &config,
                            &mut settings_ui,
                            &win_icon,
                            &window,
                            &hotkey,
                            &hotkey_manager,
                        );
                    }
                    Some("collapse") => {
                        collapse_main(&window, &mut config, &settings_ui, &mut settings_open)
                    }
                    Some("bar_ready") => {
                        let _ =
                            bar.evaluate_script(&format!("window.__qrSetPin({})", config.pin));
                    }
                    _ => {}
                }
            }
            Event::UserEvent(AppEvent::IpcSettings(s)) => {
                let Ok(v) = serde_json::from_str::<Value>(&s) else {
                    return;
                };
                match v.get("cmd").and_then(|c| c.as_str()) {
                    Some("close") => {
                        settings_open = false;
                        if let Some((w, _)) = &settings_ui {
                            w.set_visible(false);
                        }
                        resume_from_settings(&window, &hotkey, &hotkey_manager);
                    }
                    Some("drag") => {
                        if let Some((sw, _)) = &settings_ui {
                            let _ = sw.drag_window();
                        }
                    }
                    Some("save") => {
                        let (ok, msg) = apply_settings(
                            &v,
                            &mut config,
                            &mut hotkey,
                            &hotkey_manager,
                            &window,
                            &chat,
                        );
                        if ok {
                            settings_open = false;
                            if let Some((w, _)) = &settings_ui {
                                w.set_visible(false);
                            }
                            resume_from_settings(&window, &hotkey, &hotkey_manager);
                        } else if let Some((_, sv)) = &settings_ui {
                            let esc = msg.replace('\\', "\\\\").replace('\'', "\\'");
                            let _ =
                                sv.evaluate_script(&format!("window.__qrResult('{}')", esc));
                        }
                    }
                    _ => {}
                }
            }
            Event::WindowEvent { window_id, event, .. } => {
                if window_id == main_id {
                    match event {
                    WindowEvent::CloseRequested => {
                        collapse_main(&window, &mut config, &settings_ui, &mut settings_open);
                    }
                        WindowEvent::Resized(_) => {
                            layout(&window, &bar, &chat);
                        }
                        _ => {}
                    }
                } else if let Some((sw, _)) = &settings_ui {
                    if window_id == sw.id() {
                        if let WindowEvent::CloseRequested = event {
                            sw.set_visible(false);
                            settings_open = false;
                            resume_from_settings(&window, &hotkey, &hotkey_manager);
                        }
                    }
                }
            }
            Event::LoopDestroyed => {
                snapshot(&window, &mut config);
                config::save(&config);
            }
            _ => {}
        }
    });
}
