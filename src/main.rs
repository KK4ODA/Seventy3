// No console window behind the app in Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::time::Duration;

use seventy3::config::Settings;
use seventy3::ui::{App, LaunchOptions};

const HELP: &str = "\
Seventy3: panadapter and CAT control for the Kenwood TS-480

Usage: seventy3 [options]

  --demo                 Run with a simulated rig and IF (no hardware needed)
  --opengl               Use the OpenGL renderer instead of Direct3D/Vulkan/Metal
  --help                 Show this help

Developer options:
  --screenshot <file>    Save a PNG of the window, then quit
  --screenshot-after <s> Seconds to wait before the screenshot (default 6)
  --freq <Hz>            Demo rig's starting frequency
  --zoom <0-6>           Starting zoom level
";

fn main() -> eframe::Result {
    let mut launch = LaunchOptions { screenshot_after: Duration::from_secs(6), ..LaunchOptions::default() };
    let mut opengl = std::env::var_os("SEVENTY3_OPENGL").is_some();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => launch.demo = true,
            "--opengl" => opengl = true,
            "--screenshot" => launch.screenshot = args.next().map(Into::into),
            "--screenshot-after" => {
                if let Some(s) = args.next().and_then(|v| v.parse::<f64>().ok()) {
                    launch.screenshot_after = Duration::from_secs_f64(s);
                }
            }
            "--freq" => launch.demo_freq_hz = args.next().and_then(|v| v.parse().ok()),
            "--zoom" => launch.zoom = args.next().and_then(|v| v.parse().ok()),
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            other => eprintln!("Ignoring unknown option {other} (see --help)"),
        }
    }

    seventy3::logging::init();
    log::info!("Seventy3 {} starting", env!("CARGO_PKG_VERSION"));
    let settings = Settings::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Seventy3")
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([980.0, 600.0]),
        renderer: if opengl { eframe::Renderer::Glow } else { eframe::Renderer::Wgpu },
        ..Default::default()
    };
    eframe::run_native("Seventy3", options, Box::new(move |cc| Ok(Box::new(App::new(cc, settings, launch)))))
}
