#![windows_subsystem = "windows"]

use clap::Arg;
use clap::Command;
use image::DynamicImage;
use image::RgbaImage;
use log::error;
use log::info;
use nalgebra::Vector2;
use notan::app::Event;
// use piston_window::types::{Color, Matrix2d};
// use piston_window::*;
use notan::draw::*;
use notan::egui::{self, *};
use notan::prelude::keyboard::KeyCode;
use notan::prelude::mouse::MouseButton;
use notan::prelude::*;
use std::ffi::OsStr;
// use splines::{Interpolation, Spline};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

mod utils;
use utils::*;
// mod events;
#[cfg(target_os = "macos")]
mod mac;
mod net;
use net::*;
#[cfg(test)]
mod tests;
mod update;

const TOAST_TIME: f64 = 2.3;

#[notan_main]
fn main() -> Result<(), String> {
    // hack for wayland
    std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "warning");
    }
    // on debug builds, override log level
    #[cfg(debug_assertions)]
    std::env::set_var("RUST_LOG", "info");
    let _ = env_logger::try_init();

    let window_config = WindowConfig::new()
        .title(&format!("Oculante | {}", env!("CARGO_PKG_VERSION")))
        .size(1026, 600) // window's size
        // .vsync() // enable vsync
        .lazy_loop()
        .resizable() // window can be resized
        .min_size(600, 400); // Set a minimum window size

    info!("Starting oculante.");
    notan::init_with(init)
        .add_config(window_config)
        .add_config(EguiConfig)
        .add_config(DrawConfig)
        .event(event)
        .update(update)
        .draw(drawe)
        .build()
}

fn init(_gfx: &mut Graphics, plugins: &mut Plugins) -> OculanteState {
    info!("Now matching arguments {:?}", std::env::args());
    let args: Vec<String> = std::env::args().filter(|a| !a.contains("psn_")).collect();

    let matches = Command::new("Oculante")
        .arg(
            Arg::new("INPUT")
                .help("Display this image")
                // .required(true)
                .index(1),
        )
        .arg(
            Arg::new("l")
                .short('l')
                .help("Listen on port")
                .takes_value(true),
        )
        .arg(
            Arg::new("chainload")
                .required(false)
                .takes_value(false)
                .short('c')
                .help("Chainload on Mac"),
        )
        .get_matches_from(args);

    info!("Completed argument parsing.");

    let maybe_img_location = matches.value_of("INPUT").map(|arg| PathBuf::from(arg));

    let mut state = OculanteState {
        texture_channel: mpsc::channel(),
        // current_path: maybe_img_location.cloned(/),
        ..Default::default()
    };

    state.player = Player::new(state.texture_channel.0.clone());

    info!("Image is: {:?}", maybe_img_location);

    #[cfg(target_os = "macos")]
    if !matches.is_present("chainload") && maybe_img_location.is_none() {
        info!("Chainload not specified, and no input file present. Invoking mac hack.");
        // MacOS needs an incredible dance performed just to open a file
        let _ = mac::launch();
    }

    if let Some(ref img_location) = maybe_img_location {
        state.current_path = Some(img_location.clone());
        if img_location.extension() == Some(&std::ffi::OsString::from("gif")) {
            state.player.load(&img_location);
        } else {
            state.player.load_blocking(&img_location);
        }
    }

    if let Some(port) = matches.value_of("l") {
        match port.parse::<i32>() {
            Ok(p) => {
                state.message = format!("Listening on {}", p);
                recv(p, state.texture_channel.0.clone());
                state.current_path = Some(PathBuf::from(&format!("network port {p}")));
            }
            Err(_) => eprintln!("Port must be a number"),
        }
    }

    // Set up egui style
    plugins.egui(|ctx| {
        let mut fonts = FontDefinitions::default();

        fonts.font_data.insert(
            "customfont".to_owned(),
            FontData::from_static(include_bytes!("NotoSans-Regular.ttf")),
        );

        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "customfont".into());

        let mut style: egui::Style = (*ctx.style()).clone();

        style.text_styles.get_mut(&TextStyle::Body).unwrap().size = 18.;
        style.text_styles.get_mut(&TextStyle::Button).unwrap().size = 18.;
        style.text_styles.get_mut(&TextStyle::Small).unwrap().size = 15.;
        ctx.set_style(style);
        ctx.set_fonts(fonts);
    });

    state
}

fn event(state: &mut OculanteState, evt: Event) {
    match evt {
        Event::MouseWheel { delta_y, .. } => {
            let delta = zoomratio(delta_y, state.scale);
            state.offset -= scale_pt(state.offset, state.cursor, state.scale, delta);
            state.scale += delta;
        }
        Event::KeyDown { key: KeyCode::V } => state.reset_image = true,
        Event::KeyDown { key: KeyCode::Q } => std::process::exit(0),
        Event::KeyDown { key: KeyCode::I } => state.info_enabled = !state.info_enabled,
        Event::WindowResize { width, height } => {
            let window_size = (width, height).size_vec();
            if let Some(current_image) = &state.current_image {
                let img_size = current_image.size_vec();
                state.offset = window_size / 2.0 - (img_size * state.scale) / 2.0;
            }
        }
        Event::Drop(file) => {
            if let Some(p) = file.path {
                state.is_loaded = false;
                state.current_image = None;
                state.player.load(&p);
                state.current_path = Some(p);
            }
        }

        _ => {}
    }
}

fn update(app: &mut App, state: &mut OculanteState) {
    let mouse_pos = app.mouse.position();

    state.mouse_delta = Vector2::new(mouse_pos.0, mouse_pos.1) - state.cursor;
    state.cursor = mouse_pos.size_vec();

    if app.mouse.is_down(MouseButton::Left) {
        state.drag_enabled = true;

        state.offset += state.mouse_delta;
    }

    if state.info_enabled {
        state.cursor_relative = pos_from_coord(
            state.offset,
            state.cursor,
            Vector2::new(
                state.image_dimension.0 as f32,
                state.image_dimension.1 as f32,
            ),
            state.scale,
        );
    }

    if app.mouse.was_released(MouseButton::Left) {
        state.drag_enabled = false;
    }
}

fn drawe(app: &mut App, gfx: &mut Graphics, plugins: &mut Plugins, state: &mut OculanteState) {
    if state.reset_image {
        let window_size = app.window().size().size_vec();
        // let window_size = app.backend.

        if let Some(current_image) = &state.current_image {
            let img_size = current_image.size_vec();
            let scale_factor = (window_size.x / img_size.x)
                .min(window_size.y / img_size.y)
                .min(1.0);
            state.scale = scale_factor;
            state.offset = window_size / 2.0 - (img_size * state.scale) / 2.0;
            state.reset_image = false;
            info!("Done reset");
        }
        info!("fs? {}", app.window().is_fullscreen());
    }

    let mut draw = gfx.create_draw();

    if let Some(p) = &state.current_path {
        if p.extension() == Some(OsStr::new("gif")) {
            app.window().request_frame();
        }
    }

    // check if a new texture has been sent
    if let Ok(img) = state.texture_channel.1.try_recv() {
        info!("Received image buffer");
        state.image_dimension = (img.width(), img.height());
        state.current_texture = img.to_texture(gfx);

        //center the image
        state.offset = gfx.size().size_vec() / 2.0 - img.size_vec() / 2.0;

        state.reset_image = true;
        state.is_loaded = true;
        state.current_image = Some(img);
    }

    if let Some(texture) = &state.current_texture {
        draw.image(texture)
            .blend_mode(BlendMode::NORMAL)
            .translate(state.offset.x as f32, state.offset.y as f32)
            .scale(state.scale, state.scale);
    }

    let egui_output = plugins.egui(|ctx| {
        egui::TopBottomPanel::top("menu").show(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Channels");

                if ui.button("R").clicked() || app.keyboard.was_pressed(KeyCode::R) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = solo_channel(img, 0).to_texture(gfx);
                    }
                }
                if ui.button("G").clicked() || app.keyboard.was_pressed(KeyCode::G) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = solo_channel(img, 1).to_texture(gfx);
                    }
                }
                if ui.button("B").clicked() || app.keyboard.was_pressed(KeyCode::B) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = solo_channel(img, 2).to_texture(gfx);
                    }
                }
                if ui.button("A").clicked() || app.keyboard.was_pressed(KeyCode::A) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = solo_channel(img, 3).to_texture(gfx);
                    }
                }

                if ui.button("Unpremultiplied").clicked() || app.keyboard.was_pressed(KeyCode::U) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = unpremult(img).to_texture(gfx);
                    }
                }
                if ui.button("RGBA").clicked() || app.keyboard.was_pressed(KeyCode::C) {
                    if let Some(img) = &state.current_image {
                        state.current_texture = img.to_texture(gfx);
                    }
                }

                ui.add(egui::Separator::default().vertical());

                if ui.button("⛶").clicked() || app.keyboard.was_pressed(KeyCode::F) {
                    let fullscreen = app.window().is_fullscreen();
                    app.window().set_fullscreen(!fullscreen);
                    // state.reset_image = true;
                }

                ui.checkbox(&mut state.info_enabled, "Show extended info");

                if ui.button("◀").clicked() || app.keyboard.was_pressed(KeyCode::Left) {
                    if let Some(img_location) = state.current_path.as_mut() {
                        let next_img = img_shift(&img_location, -1);
                        // prevent reload if at last or first
                        if &next_img != img_location {
                            state.is_loaded = false;
                            *img_location = next_img;
                            state.player.load(&img_location);
                            // set_title(&mut window, &img_location.to_string_lossy().to_string());
                        }
                    }
                }
                if ui.button("▶").clicked() || app.keyboard.was_pressed(KeyCode::Right) {
                    if let Some(img_location) = state.current_path.as_mut() {
                        let next_img = img_shift(&img_location, 1);
                        // prevent reload if at last or first
                        if &next_img != img_location {
                            state.is_loaded = false;
                            *img_location = next_img;
                            state.player.load(&img_location);
                            // set_title(&mut window, &img_location.to_string_lossy().to_string());
                        }
                    }
                }
            });
        });

        if state.info_enabled {
            egui::SidePanel::left("side_panel").show(&ctx, |ui| {
                ui.label(format!(
                    "Size: {}x{}",
                    state.image_dimension.0, state.image_dimension.1
                ));

                if let Some(path) = &state.current_path {
                    ui.label(format!("Path: {}", path.display()));
                }

                if let Some(img) = &state.current_image {
                    if let Some(p) = img.get_pixel_checked(
                        state.cursor_relative.x as u32,
                        state.cursor_relative.y as u32,
                    ) {
                        state.sampled_color = [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32];
                    }
                }

                if let Some(texture) = &state.current_texture {
                    let desired_width = 200.;
                    let img_size: egui::Vec2 = texture.size().into();
                    let scale = desired_width / img_size.x;
                    let img_size = img_size * scale;

                    let tex_id = gfx.egui_register_texture(&texture);

                    let uv = (
                        state.cursor_relative.x / state.image_dimension.0 as f32,
                        state.cursor_relative.y / state.image_dimension.1 as f32,
                    );

                    ui.label(format!("UV: {:.3},{:.3}", uv.0, uv.1));
                    ui.label(format!(
                        "PX: {:.0},{:.0}",
                        state.cursor_relative.x, state.cursor_relative.y
                    ));
                    ui.label(format!("CLR: {:?}", state.sampled_color));
                    ui.add(
                        egui::Image::new(tex_id, img_size)
                            .uv(egui::Rect::from_x_y_ranges(
                                uv.0 - 0.1..=uv.0 + 0.1,
                                uv.1 - 0.1..=uv.1 + 0.1,
                            ))
                            .bg_fill(egui::Color32::GRAY),
                    );
                    // ui.image(tex_id, img_size);
                }
            });
        }

        if !state.is_loaded {
            egui::Window::new("")
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .default_width(400.)
                .title_bar(false)
                .show(&ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::default());

                        ui.label(format!(
                            "Loading {}",
                            state.current_path.clone().unwrap_or_default().display()
                        ));
                    });
                });
        }
    });

    // output.clear_color(Color::BLACK);

    draw.clear(Color::from_rgb(0.2, 0.2, 0.2));
    gfx.render(&draw);
    gfx.render(&egui_output);
    if egui_output.needs_repaint() {
        app.window().request_frame();
    }

    // redraw constantly until the image is fully loaded or it's reset on canvas
    if !state.is_loaded || state.reset_image {
        app.window().request_frame();
    }
}

// fn set_title(window: &mut PistonWindow, text: &str) {
//     let title = format!("Oculante {} | {}", env!("CARGO_PKG_VERSION"), text);
//     window.set_title(title);
// }

fn _init(gfx: &mut Graphics) {
    //update::update();

    let mut state = OculanteState::default();
    let mut toast_time = std::time::Instant::now();

    info!("Now matching arguments {:?}", std::env::args());
    let args: Vec<String> = std::env::args().filter(|a| !a.contains("psn_")).collect();

    let matches = Command::new("Oculante")
        .arg(
            Arg::new("INPUT")
                .help("Display this image")
                // .required(true)
                .index(1),
        )
        .arg(
            Arg::new("l")
                .short('l')
                .help("Listen on port")
                .takes_value(true),
        )
        .arg(
            Arg::new("chainload")
                .required(false)
                .takes_value(false)
                .short('c')
                .help("Chainload on Mac"),
        )
        .get_matches_from(args);

    info!("Completed argument parsing.");

    let font_regular = include_bytes!("IBMPlexSans-Regular.ttf");

    // animation
    // let k1 = splines::Key::new(0., 0., Interpolation::Cosine);
    // let k2 = splines::Key::new(TOAST_TIME * 0.125, 80., Interpolation::default());
    // let k3 = splines::Key::new(TOAST_TIME * 0.875, 80., Interpolation::default());
    // let k4 = splines::Key::new(TOAST_TIME, 0., Interpolation::default());
    // let spline = Spline::from_vec(vec![k1, k2, k3, k4]);

    let mut maybe_img_location = matches.value_of("INPUT").map(|arg| PathBuf::from(arg));

    let (texture_sender, texture_receiver): (Sender<RgbaImage>, Receiver<RgbaImage>) =
        mpsc::channel();

    let (toast_sender, toast_receiver): (Sender<String>, Receiver<String>) = mpsc::channel();

    let player = Player::new(texture_sender.clone());

    info!("Image is: {:?}", maybe_img_location);

    #[cfg(target_os = "macos")]
    if !matches.is_present("chainload") && maybe_img_location.is_none() {
        info!("Chainload not specified, and no input file present. Invoking mac hack.");
        // MacOS needs an incredible dance performed just to open a file
        let _ = mac::launch();
    }

    if let Some(ref img_location) = maybe_img_location {
        if img_location.extension() == Some(&std::ffi::OsString::from("gif")) {
            player.load(&img_location);
        } else {
            player.load_blocking(&img_location);
        }
    }

    // let ws = WindowSettings::new("Oculante", [1000, 800])
    //     .graphics_api(opengl_version)
    //     .fullscreen(false)
    //     .vsync(true)
    //     .exit_on_esc(true);

    // let mut window: PistonWindow = ws.build().unwrap();
    // set_title(&mut window, "No image");

    // #[cfg(target_os = "macos")]
    // let scale_factor = window.draw_size().width / window.size().width;

    // Set inspection-friendly magnification filter
    // let mut tx_settings = TextureSettings::new();
    // tx_settings.set_mag(Filter::Nearest);
    // tx_settings.set_min(Filter::Linear);

    // These should all be a nice config struct...
    let mut current_image = DynamicImage::new_rgba8(8, 8).to_rgba8();
    let mut texture = gfx.create_texture().with_size(256, 256).build().unwrap();

    if let Some(img_location) = maybe_img_location.as_ref() {
        if img_location.is_file() {
            state.message = "Loading...".to_string();
            // TODO
            // set_title(&mut window, &img_location.to_string_lossy().to_string());
        }
    }

    if let Some(port) = matches.value_of("l") {
        match port.parse::<i32>() {
            Ok(p) => {
                state.message = format!("Listening on {}", p);
                recv(p, texture_sender);
            }
            Err(_) => eprintln!("Port must be a number"),
        }
    }

    let _ = toast_sender
        .clone()
        .send("Press 'h' to toggle help!".to_string());
}

// fn _main(gfx: &mut Graphics) {
//     // Event loop
//     while let Some(e) = window.next() {
//         // check if a new texture has been sent
//         if let Ok(img) = texture_receiver.try_recv() {
//             window.set_lazy(false);

//             texture = Texture::from_image(&mut window.create_texture_context(), &img, &tx_settings);
//             current_image = img;

//             let draw_size = window.size();

//             let window_size = Vector2::new(draw_size.width, draw_size.height);
//             let img_size =
//                 Vector2::new(current_image.width() as f64, current_image.height() as f64);
//             state.offset = window_size / 2.0 - img_size / 2.0;
//             state.reset_image = true;
//             state.is_loaded = true;
//         }

//         // Receive a dragged file
//         if let Event::Input(Input::FileDrag(FileDrag::Drop(p)), None) = &e {
//             window.set_lazy(false);
//             state.message = "Loading...".to_string();
//             state.is_loaded = false;
//             player.load(&p);
//             set_title(&mut window, &p.to_string_lossy().to_string());

//             maybe_img_location = Some(p.clone());
//         }

//         // Initiate a pan operation on any button
//         if let Some(Button::Mouse(_)) = e.press_args() {
//             state.drag_enabled = true;
//             state.cursor_relative = pos_from_coord(
//                 state.offset,
//                 state.cursor,
//                 Vector2::new(
//                     state.image_dimension.0 as f64,
//                     state.image_dimension.1 as f64,
//                 ),
//                 state.scale,
//             );
//             // state.sampled_color = current_image.get_pixel(state.cursor_relative.x as u32, state.cursor_relative.y as u32).channels4();
//         }

//         //handle_events(&mut state, &e);

//         if let Some(Button::Mouse(_)) = e.release_args() {
//             state.drag_enabled = false;
//         }

//         if let Some(Button::Keyboard(key)) = e.press_args() {
//             if key == Key::V {
//                 state.reset_image = true;
//             }

//             // Quit
//             if key == Key::Q {
//                 std::process::exit(0);
//             }

//             // Set state.fullscreen_enabled
//             if key == Key::F {
//                 if !state.fullscreen_enabled {
//                     window.set_size([1920, 1080]);
//                     window = ws.clone().fullscreen(true).build().unwrap();
//                 } else {
//                     window = ws.clone().fullscreen(false).build().unwrap();
//                 }

//                 // state.reset_image = true;
//                 texture = Texture::from_image(
//                     &mut window.create_texture_context(),
//                     &current_image,
//                     &tx_settings,
//                 );

//                 glyphs_regular = Glyphs::from_bytes(
//                     font_regular,
//                     window.create_texture_context(),
//                     TextureSettings::new(),
//                 )
//                 .unwrap();

//                 state.fullscreen_enabled = !state.fullscreen_enabled;
//                 // pause so we don't enter a fullscreen loop - otherwise some OSes crash
//                 std::thread::sleep(std::time::Duration::from_millis(100));
//             }

//             // Toggle extended info
//             if key == Key::I {
//                 state.info_enabled = !state.info_enabled;
//             }

//             // Toggle extended info
//             if key == Key::D1 {
//                 state.scale = window.size().width / window.draw_size().width;
//                 let window_size = Vector2::new(window.size().width, window.size().height);
//                 let img_size =
//                     Vector2::new(current_image.width() as f64, current_image.height() as f64);
//                 state.offset = window_size / 2.0 - (img_size * state.scale) / 2.0;
//             }

//             // Toggle tooltip
//             if key == Key::H {
//                 state.tooltip = !state.tooltip;
//             }

//             // Next image
//             if key == Key::Right {
//                 info!("right");
//                 if let Some(img_location) = maybe_img_location.as_mut() {
//                     info!("| {:?}", img_location);

//                     let next_img = img_shift(&img_location, 1);
//                     info!("|> {:?}", next_img);

//                     // prevent reload if at last or first
//                     if &next_img != img_location {
//                         state.reset_image = true;
//                         window.set_lazy(false);
//                         state.is_loaded = false;
//                         *img_location = next_img;
//                         player.load(&img_location);
//                         set_title(&mut window, &img_location.to_string_lossy().to_string());
//                     }
//                 }
//             }

//             // Prev image
//             if key == Key::Left {
//                 if let Some(img_location) = maybe_img_location.as_mut() {
//                     let next_img = img_shift(&img_location, -1);
//                     // prevent reload if at last or first
//                     if &next_img != img_location {
//                         state.reset_image = true;
//                         window.set_lazy(false);
//                         state.is_loaded = false;
//                         *img_location = next_img;
//                         player.load(&img_location);
//                         set_title(&mut window, &img_location.to_string_lossy().to_string());
//                     }
//                 }
//             }

//             if key == Key::Comma {
//                 update::update(toast_sender.clone());
//             }
//         };

//         // TODO: rate-limit zoom (for trackpads, as they fire zoom events really fast)
//         // TODO: clamp cursor position to image bounds for zoom
//         e.mouse_scroll(|d| {
//             // Map zoom nicely so it does not feel awkward whan zoomed out/in
//             let delta = zoomratio(d[1], state.scale);
//             // prevent negative / small zoom
//             if delta + state.scale < 0.1 {
//                 return;
//             }
//             // make sure we zoom to the mouse cursor
//             state.offset -= scale_pt(state.offset, state.cursor, state.scale, delta);
//             state.scale += delta;
//         });

//         e.mouse_relative(|d| {
//             if state.drag_enabled {
//                 state.offset += Vector2::new(d[0] / scale_factor, d[1] / scale_factor);
//             }
//         });

//         e.mouse_cursor(|d| {
//             state.cursor = Vector2::new(d[0] / scale_factor, d[1] / scale_factor);
//             state.cursor_relative = pos_from_coord(
//                 state.offset,
//                 state.cursor,
//                 Vector2::new(
//                     state.image_dimension.0 as f64,
//                     state.image_dimension.1 as f64,
//                 ),
//                 state.scale,
//             );
//             if state.cursor_relative.x as u32 <= current_image.width()
//                 && state.cursor_relative.y as u32 <= current_image.height()
//                 && state.info_enabled
//             {
//                 let p = current_image
//                     .get_pixel(
//                         state.cursor_relative.x as u32,
//                         state.cursor_relative.y as u32,
//                     )
//                     .channels4();
//                 state.sampled_color = [p.0 as f32, p.1 as f32, p.2 as f32, p.3 as f32];
//             }
//         });

//         // e.resize(|args| {
//         //     println!("Resized '{}, {}'", args.window_size[0], args.window_size[1])
//         // });

//         let size = window.size();

//         window.draw_2d(&e, |c, gfx, device| {
//             clear([0.2; 4], gfx);

//             if state.reset_image {
//                 let window_size = Vector2::new(size.width, size.height);
//                 let img_size =
//                     Vector2::new(current_image.width() as f64, current_image.height() as f64);
//                 let scale_factor = (window_size.x / img_size.x).min(window_size.y / img_size.y).min(1.0);
//                 state.scale = scale_factor;cursor_relative
//                 state.offset = Vector2::new(0.0, 0.0);
//                 state.offset += window_size / 2.0 - (img_size * state.scale) / 2.0;
//                 state.reset_image = false;
//             }

//             let transform = c
//                 .transform
//                 .trans(state.offset.x as f64, state.offset.y as f64)
//                 .zoom(state.scale);

//             // draw the image
//             if let Ok(tex) = &texture {
//                 image(tex, transform, gfx);
//                 state.image_dimension = tex.get_size();
//             }

//             let default_path = PathBuf::from("No image");
//             let filename = maybe_img_location.as_ref().unwrap_or(&default_path).file_name().unwrap_or_default().to_string_lossy();
//             let info = format!(
//                 "{} {}X{} @{}X",
//                 filename,
//                 state.image_dimension.0,
//                 state.image_dimension.1,
//                 (state.scale * 10.0).round() / 10.0
//             );

//             if state.info_enabled {
//                 draw_text(&c, gfx, &mut glyphs_regular, &TextInstruction::new(
//                     &info,
//                     (10.0, 20.0),
//                 ));
//             }

//             if !state.is_loaded {
//                 draw_text(&c, gfx, &mut glyphs_regular, &TextInstruction::new_size(
//                     &state.message,
//                     (size.width / 2.0 - 120.0, size.height / 2.0),
//                     state.font_size * 2,
//                 ));
//             }

//             if state.info_enabled {
//                 let col_inv = invert_rgb_8bit(state.sampled_color);

//                 // draw the zoomed image
//                 if let Ok(tex) = &texture {
//                     let rect_size = 128.0;
//                     let cur = state.cursor;
//                     let mut cur_relative = state.cursor_relative;
//                     // Snap relative mouse position so we see the exact pixel position
//                     cur_relative.x = cur_relative.x.floor();
//                     cur_relative.y = cur_relative.y.floor();

//                     let cropped_res = 16.0;
//                     let image = Image::new()
//                         .src_rect([
//                             cur_relative.x - cropped_res / 2.,
//                             cur_relative.y - cropped_res / 2.,
//                             cropped_res,
//                             cropped_res,
//                         ])
//                         .rect([0.0, 0.0, rect_size, rect_size]);
//                     let t_cursor = c.transform.trans(cur.x, cur.y).zoom(1.0);

//                     // Draw the picker window
//                     image.draw(tex, &draw_state::DrawState::default(), t_cursor, gfx);

//                     let t_rect_center = c
//                         .transform
//                         .trans(cur.x + rect_size / 2., cur.y + rect_size / 2.)
//                         .zoom(1.0);

//                     // A small rect covering the picked pixel
//                     let pixel_rect = Rectangle::new(col_inv);
//                     // A frame over the magnified texture
//                     let frame = Rectangle::new_border([0.0, 0.0, 0.0, 0.5], 1.);
//                     frame.draw(
//                         [0.0, 0.0, rect_size, rect_size],
//                         &draw_state::DrawState::default(),
//                         t_cursor,
//                         gfx,
//                     );
//                     pixel_rect.draw(
//                         [0.0, 0.0, cropped_res / 2., cropped_res / 2.],
//                         &draw_state::DrawState::default(),
//                         t_rect_center,
//                         gfx,
//                     );
//                 }

//                 draw_text(&c, gfx, &mut glyphs_regular,&TextInstruction::new(
//                     &format!(
//                         "P {},{} / {},{}",
//                         state.cursor_relative[0].floor() as i32 + 1,
//                         state.image_dimension.1 as i32 - (state.cursor_relative[1].floor() as i32),
//                         state.cursor_relative[0].floor() as i32 + 1,
//                         state.cursor_relative[1].floor() as i32 + 1,
//                     ),
//                     (state.cursor.x, state.cursor.y - 4.),
//                 ));

//                 draw_text(&c, gfx, &mut glyphs_regular,&TextInstruction::new(
//                     &format!(
//                         "C {} / {}",
//                         disp_col(state.sampled_color),
//                         disp_col_norm(state.sampled_color, 255.0),
//                     ),
//                     (
//                         state.cursor.x,
//                         state.cursor.y - state.font_size as f64 * 1.5,
//                     ),
//                 ));
//             }

//             // The toast system
//             {
//                 if let Ok(toast) = toast_receiver.try_recv() {
//                     toast_time = std::time::Instant::now();
//                     state.toast = toast;
//                 }
//                 if state.toast != "" {
//                     let elapsed = toast_time.elapsed().as_secs_f64();

//                     if elapsed < TOAST_TIME {
//                         let text_rect = Rectangle::new([0.,0.,0.,0.7]);
//                         text_rect.draw(
//                             [0., 0., size.width, -100.],
//                             &draw_state::DrawState::default(),
//                             c.transform.trans(0., spline.clamped_sample(elapsed).unwrap() + 10.),
//                             gfx,
//                         );

//                         let text_size = text::Text::new( 14).width(&state.toast,&mut glyphs_regular);

//                         draw_text(&c, gfx, &mut glyphs_regular,&TextInstruction::new(
//                             &state.toast,
//                     (size.width/2. - text_size.0/2., spline.clamped_sample(elapsed).unwrap()),
//                         ));
//                     }
//                 }
//             }

//             glyphs_regular.factory.encoder.flush(device);
//         });

//         // if let Ok(state_msg) = state_receiver.try_recv() {
//         //     // an image has been received
//         //     // window.set_lazy(false);
//         //     state.is_loaded = true;

//         //     if state_msg != "ANIM_FRAME" {
//         //         state.reset_image = true;
//         //         window.set_lazy(true);
//         //     } else {
//         //     }

//         // }
//     }
// }

// pub fn draw_text(
//     ctx: &Context,
//     graphics: &mut G2d,
//     glyphs: &mut Glyphs,
//     instructions: &TextInstruction,
// ) {
//     let text_rect = Rectangle::new([0., 0., 0., 0.5 * instructions.color[3]]);

//     let text = text::Text::new_color(instructions.color, instructions.size);

//     let text_width = text.width(&instructions.text, glyphs);

//     let margin = 5.;
//     text_rect.draw(
//         [
//             -margin,
//             margin,
//             text_width.0 + margin * 2.,
//             -(text_width.1 + margin),
//         ],
//         &draw_state::DrawState::default(),
//         ctx.transform.trans(
//             instructions.position.0 as f64,
//             instructions.position.1 as f64,
//         ),
//         graphics,
//     );

//     text.draw(
//         &instructions.text,
//         glyphs,
//         &ctx.draw_state,
//         ctx.transform.trans(
//             instructions.position.0 as f64,
//             instructions.position.1 as f64,
//         ),
//         graphics,
//     )
//     .unwrap();
// }
