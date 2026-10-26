//! Novo.app (Tauri 2): janela dos espaços servida pelo `novod`, ícone na barra de menus e atalho global
//! ⌘⇧N para publicar com o link do clipboard (RF-PUB-05). Toda a lógica fica no daemon; o app é fino (ADR "Interface").

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use novo_core::Vault;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

fn novod_bin() -> PathBuf {
    if let Ok(p) = std::env::var("NOVOD_BIN") {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        for name in ["novod", "novod-aarch64-apple-darwin", "novod-x86_64-apple-darwin"] {
            let sib = exe.with_file_name(name);
            if sib.exists() {
                return sib;
            }
        }
    }
    "novod".into()
}

/// Garante o daemon rodando (ele pode já estar no LaunchAgent) e devolve a URL da interface com o token.
fn ensure_daemon(vault: &Vault) -> Result<String, String> {
    if UnixStream::connect(vault.socket_path()).is_err() {
        std::process::Command::new(novod_bin())
            .env("NOVO_HOME", &vault.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("não consegui iniciar o novod: {e}"))?;
        for _ in 0..100 {
            if UnixStream::connect(vault.socket_path()).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    for _ in 0..50 {
        let port = std::fs::read_to_string(vault.runtime_dir().join("http.port")).unwrap_or_default();
        let token = std::fs::read_to_string(vault.runtime_dir().join("token")).unwrap_or_default();
        if !port.trim().is_empty() && token.trim().len() >= 32 {
            return Ok(format!("http://127.0.0.1:{}/", port.trim()) + &format!("#token={}", token.trim()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("o novod não publicou a porta HTTP".into())
}

fn base_of(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_string()
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// O que estiver no clipboard, se parecer um link.
fn clipboard_link() -> String {
    let out = std::process::Command::new("pbpaste").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if (out.starts_with("http://") || out.starts_with("https://")) && !out.contains(char::is_whitespace) { out } else { String::new() }
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Compositor rápido (RF-PUB-05): janela pequena, com o link do clipboard já preenchido.
fn open_compose(app: &AppHandle, url: &str) {
    let target = format!("{}#/compose?text={}", base_of(url), encode(&clipboard_link()));
    if let Some(w) = app.get_webview_window("compose") {
        let _ = w.eval(&format!("location.replace({:?})", target));
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let handle = app.clone();
    let close_url = format!("{}__close", base_of(url));
    let built = WebviewWindowBuilder::new(app, "compose", WebviewUrl::External(target.parse().expect("url")))
        .title("Publicar no Novo")
        .inner_size(560.0, 300.0)
        .resizable(false)
        .always_on_top(true)
        .center()
        .on_navigation(move |u| {
            // a página pede para fechar navegando para /__close
            if u.as_str().starts_with(&close_url) {
                if let Some(w) = handle.get_webview_window("compose") {
                    let _ = w.hide();
                }
                return false;
            }
            true
        })
        .build();
    if let Ok(w) = built {
        let _ = w.set_focus();
    }
}

fn main() {
    let vault = match Vault::from_env() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cofre: {e:#}");
            std::process::exit(1);
        }
    };
    let url = match ensure_daemon(&vault) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let quick = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyN);
    let url_for_keys = url.clone();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, sc, ev| {
                    if sc == &quick && ev.state() == ShortcutState::Pressed {
                        open_compose(app, &url_for_keys);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse().expect("url")))
                .title("Anthive Novo")
                .inner_size(1240.0, 820.0)
                .min_inner_size(420.0, 500.0)
                .build()?;
            let open = MenuItem::with_id(app, "open", "Abrir o Novo", true, None::<&str>)?;
            let compose = MenuItem::with_id(app, "compose", "Publicar rápido (⌘⇧N)", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &compose, &quit])?;
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            let url_for_tray = url.clone();
            TrayIconBuilder::with_id("novo")
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("Anthive Novo")
                .menu(&menu)
                .on_menu_event(move |app, ev| match ev.id().as_ref() {
                    "open" => show_main(app),
                    "compose" => open_compose(app, &url_for_tray),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            if let Err(e) = app.global_shortcut().register(Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyN)) {
                eprintln!("atalho global ⌘⇧N indisponível: {e}");
            }
            Ok(())
        })
        .on_window_event(|w, ev| {
            // fechar a janela só esconde: o Novo continua na barra de menus
            if let WindowEvent::CloseRequested { api, .. } = ev {
                let _ = w.hide();
                api.prevent_close();
            }
            if w.label() == "compose" {
                if let WindowEvent::Focused(false) = ev {
                    let _ = w.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao rodar o Novo.app");
}
