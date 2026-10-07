//! Blackwall: a cyberpunk system visualizer.
//!
//! ```text
//! blackwall [--demo] [--interval-ms N] [--quality low|medium|high|ultra]
//!           [--size WxH] [--select NAME] [--hide-ui] [--no-intro]
//!           [--screenshot PATH [--after SECONDS]]
//! ```

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::{PresentMode, WindowResolution};
use bevy_egui::EguiPlugin;
use bw_scene::{Machine, Quality, ScenePlugin, Selection, SourceRx, Tier};
use bw_source::SourceKind;
use bw_ui::{UiPlugin, UiState};
use std::time::Duration;

#[derive(Debug, Clone)]
struct Args {
    demo: bool,
    interval: Duration,
    quality: Option<Tier>,
    size: (u32, u32),
    select: Option<String>,
    hide_ui: bool,
    no_intro: bool,
    screenshot: Option<String>,
    after: f32,
}

fn usage() -> ! {
    eprintln!(
        "usage: blackwall [--demo] [--interval-ms N] [--quality low|medium|high|ultra] [--size WxH]\n                 [--select NAME] [--hide-ui] [--no-intro] [--screenshot PATH [--after SECONDS]]"
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        demo: false,
        interval: Duration::from_millis(1000),
        quality: None,
        size: (1600, 900),
        select: None,
        hide_ui: false,
        no_intro: false,
        screenshot: None,
        after: 8.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--demo" => a.demo = true,
            "--interval-ms" => {
                a.interval = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--quality" => a.quality = Some(Tier::parse(&val()).unwrap_or_else(|| usage())),
            "--size" => {
                let v = val();
                let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
                a.size = (
                    w.parse().unwrap_or_else(|_| usage()),
                    h.parse().unwrap_or_else(|_| usage()),
                );
            }
            "--select" => a.select = Some(val()),
            "--hide-ui" => a.hide_ui = true,
            "--no-intro" => a.no_intro = true,
            "--screenshot" => a.screenshot = Some(val()),
            "--after" => a.after = val().parse().unwrap_or_else(|_| usage()),
            "-h" | "--help" => usage(),
            "-V" | "--version" => {
                println!("blackwall {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0)
            }
            _ => usage(),
        }
    }
    a
}

fn main() -> AppExit {
    let args = parse_args();
    let kind = if args.demo {
        SourceKind::Demo
    } else {
        SourceKind::Live
    };

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Blackwall".into(),
            resolution: WindowResolution::new(args.size.0, args.size.1),
            present_mode: PresentMode::AutoVsync,
            ..default()
        }),
        ..default()
    }))
    .add_plugins((EguiPlugin::default(), ScenePlugin, UiPlugin))
    .insert_resource(Quality::new(args.quality))
    .insert_resource(SourceRx(bw_source::spawn(kind, args.interval)))
    .insert_resource({
        let mut ui = UiState::default();
        ui.hidden = args.hide_ui;
        ui
    })
    .insert_resource(bw_scene::camera::JackIn::new(!args.no_intro))
    .insert_resource(LaunchArgs(args.clone()));

    if args.select.is_some() {
        app.add_systems(Update, preselect);
    }
    if args.screenshot.is_some() {
        app.add_systems(Update, auto_screenshot);
    }
    app.run()
}

#[derive(Resource)]
struct LaunchArgs(Args);

/// `--select NAME`: select the busiest process with that name once data arrives.
fn preselect(
    args: Res<LaunchArgs>,
    m: Res<Machine>,
    mut sel: ResMut<Selection>,
    mut done: Local<bool>,
) {
    if *done || !m.received {
        return;
    }
    *done = true;
    let name = args.0.select.as_deref().unwrap_or_default();
    sel.key = m
        .snapshot
        .processes
        .values()
        .filter(|p| p.name == name)
        .max_by(|a, b| a.cpu_pct.total_cmp(&b.cpu_pct))
        .map(|p| p.key);
    if sel.key.is_none() {
        warn!("--select: no process named {name:?}");
    }
}

/// `--screenshot PATH`: capture the window `--after` seconds after the first
/// data arrives, then exit. Used for docs images and CI render smoke tests.
fn auto_screenshot(
    mut commands: Commands,
    args: Res<LaunchArgs>,
    m: Res<Machine>,
    time: Res<Time>,
    mut since: Local<Option<f32>>,
    mut taken: Local<bool>,
) {
    if *taken || !m.received {
        return;
    }
    let start = *since.get_or_insert(time.elapsed_secs());
    if time.elapsed_secs() - start < args.0.after {
        return;
    }
    *taken = true;
    let path = args.0.screenshot.clone().unwrap_or_default();
    info!("capturing screenshot to {path}");
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path))
        .observe(
            |_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            },
        );
}
