// Hide the console window on Windows release builds (GUI app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod balance;
mod classic;
mod config;
mod dock;
mod eq;
mod library;
mod player;
mod playlist;
mod region;
mod skin;
mod tags;
mod text;
mod viz;

use app::LinAmpApp;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Headless skin colour verifier: `linamp --check-skin a.wsz b.wsz ...`
    if args.iter().any(|a| a == "--check-skin") {
        for p in args.iter().skip_while(|a| *a != "--check-skin").skip(1) {
            skin::describe(std::path::Path::new(p));
        }
        return Ok(());
    }

    // Headless classic-render preview: `linamp --render-skin in.wsz out.png`
    // or `--render-all` for main+EQ+playlist stacked.
    for (flag, all) in [("--render-skin", false), ("--render-all", true)] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            let inp = args.get(i + 1);
            let out = args.get(i + 2);
            if let (Some(inp), Some(out)) = (inp, out) {
                let (inp, out) = (std::path::Path::new(inp), std::path::Path::new(out));
                let r = if all {
                    classic::render_all_png(inp, out)
                } else {
                    classic::render_main_png(inp, out)
                };
                match r {
                    Ok(()) => println!("rendered {} -> {}", inp.display(), out.display()),
                    Err(e) => println!("render error: {e}"),
                }
            } else {
                println!("usage: linamp {flag} <in.wsz> <out.png>");
            }
            return Ok(());
        }
    }

    // Headless Media Library frame preview: `linamp --render-gen in.wsz out.png`.
    if let Some(i) = args.iter().position(|a| a == "--render-gen") {
        if let (Some(inp), Some(out)) = (args.get(i + 1), args.get(i + 2)) {
            let (inp, out) = (std::path::Path::new(inp), std::path::Path::new(out));
            match classic::render_gen_png(inp, out) {
                Ok(()) => println!("rendered gen {} -> {}", inp.display(), out.display()),
                Err(e) => println!("render error: {e}"),
            }
        } else {
            println!("usage: linamp --render-gen <in.wsz> <out.png>");
        }
        return Ok(());
    }

    // Start borderless at classic skin size (the app sets the real mode on the
    // first frame once it knows whether a skin loaded).
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([550.0, 232.0])
        .with_decorations(false)
        .with_resizable(false)
        .with_title("LinAmp");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        // The classic UI uses several borderless child windows rendered as
        // immediate viewports. With vsync on, each frame blocks ~16ms on the
        // first child window's buffer swap, pegging the main thread and making
        // input feel laggy. We throttle repaints ourselves (30fps when active,
        // 4fps idle), so vsync buys nothing here — turn it off.
        vsync: false,
        ..Default::default()
    };

    eframe::run_native(
        "LinAmp",
        options,
        Box::new(|cc| Ok(Box::new(LinAmpApp::new(cc)))),
    )
}

/// Decode the embedded LinAmp logo into an egui window/taskbar icon.
fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/linamp_logo_256.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}
