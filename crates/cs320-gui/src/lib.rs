//! Window of the Corstone-320 simulator: the HDLCD (and VSI) display output, the MPS4 board
//! strip (LEDs, buttons, switches), the VIO signals and values, the camera and microphone
//! sources, the speaker, six UART terminals and the simulation controls.

mod audio;

use crossbeam_channel::Receiver;
use cs320_board::runtime::{Cmd, Input, Simulator, Speed, Status};
use cs320_board::sources::{CameraSource, MicSource, Rgba};
use cs320_core::time::to_secs;
use cs320_core::Ticks;
use egui::{Color32, ColorImage, Key, RichText, Sense, TextureHandle, TextureOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct GuiOptions {
    pub title: String,
}

/// Runs the GUI on the current (main) thread until the window closes.
pub fn run(sim: Simulator, opts: GuiOptions) -> Result<(), String> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(opts.title.clone())
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let title = opts.title.clone();
    eframe::run_native(&title, native, Box::new(move |cc| Ok(Box::new(App::new(cc, sim))))).map_err(|e| e.to_string())?;
    Ok(())
}

const DISPLAY_NAMES: [&str; 3] = ["HDLCD", "VSI6", "VSI7"];
const UARTS: usize = 6;
const LED_ON: Color32 = Color32::from_rgb(0x40, 0xe0, 0x50);
const LED_OFF: Color32 = Color32::from_rgb(0x2c, 0x3a, 0x2e);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zoom {
    Fit,
    X1,
    X2,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineEnd {
    None,
    Lf,
    Cr,
    CrLf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CamSel {
    TestPattern,
    None,
    Image,
    Folder,
    Live,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MicSel {
    Silence,
    Tone,
    Wav,
    Live,
}

/// One display output as a texture; the image is kept to re-upload it with another filter.
#[derive(Default)]
struct Display {
    tex: Option<TextureHandle>,
    image: Option<ColorImage>,
    nearest: bool,
}

struct App {
    sim: Simulator,
    status: Status,
    // display
    displays: Vec<Display>,
    display: usize,
    zoom: Zoom,
    // board inputs (GUI side)
    buttons: [bool; 2],
    mouse_buttons: [bool; 2],
    key_buttons: [bool; 2],
    switches: u8,
    vio_buttons: [bool; 4],
    vio_mouse: [bool; 4],
    vio_in: [i32; 4],
    // camera
    cam_sel: CamSel,
    cam_path: String,
    cam_tex: Option<TextureHandle>,
    cam_note: Option<String>,
    /// The host webcam while the live source is selected (dropping it stops ffmpeg).
    webcam: Option<cs320_board::sources::Webcam>,
    // microphone
    mic_sel: MicSel,
    tone_hz: f64,
    tone_amp: f64,
    wav_path: String,
    mic_noise: f64,
    live_mic: Option<audio::LiveMic>,
    mic_note: Option<String>,
    // speaker
    speaker: audio::Speaker,
    // terminals
    uart_rx: Vec<Receiver<Vec<u8>>>,
    terminals: Vec<String>,
    tab: usize,
    input_line: String,
    line_end: LineEnd,
    // misc
    last_time: Ticks,
    message: Option<(String, Instant)>,
    show_warnings: bool,
    /// `CS320_SCREENSHOT=path.png[:seconds]`: save a screenshot and exit (documentation, tests).
    screenshot: Option<(String, Instant, bool)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, sim: Simulator) -> App {
        // dark whatever the system theme is
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        // the target boots before the window opens: start with what the UARTs already sent
        let mut uart_rx = Vec::with_capacity(UARTS);
        let mut terminals = Vec::with_capacity(UARTS);
        for n in 0..UARTS {
            let (history, rx) = sim.subscribe_uart_with_history(n);
            uart_rx.push(rx);
            terminals.push(String::from_utf8_lossy(&history).replace('\r', ""));
        }
        let status = sim.status();
        let inputs = status.inputs.clone().unwrap_or_default();
        let mut vio_in = [0i32; 4];
        for (id, v) in &inputs.vio_values {
            if *id < 4 {
                vio_in[*id] = *v;
            }
        }
        App {
            sim,
            status,
            displays: (0..3).map(|_| Display::default()).collect(),
            display: 0,
            zoom: Zoom::Fit,
            buttons: inputs.buttons,
            mouse_buttons: [false; 2],
            key_buttons: [false; 2],
            switches: inputs.switches,
            vio_buttons: [false; 4],
            vio_mouse: [false; 4],
            vio_in,
            cam_sel: CamSel::TestPattern,
            cam_path: String::new(),
            cam_tex: None,
            cam_note: None,
            webcam: None,
            mic_sel: MicSel::Silence,
            tone_hz: 1000.0,
            tone_amp: 0.5,
            wav_path: String::new(),
            mic_noise: 0.0,
            live_mic: None,
            mic_note: None,
            speaker: audio::Speaker::new(),
            uart_rx,
            terminals,
            tab: 0,
            input_line: String::new(),
            line_end: LineEnd::Lf,
            last_time: 0,
            message: None,
            show_warnings: false,
            screenshot: std::env::var("CS320_SCREENSHOT").ok().map(|v| {
                let (path, secs) = match v.rsplit_once(':') {
                    Some((p, s)) if s.parse::<f64>().is_ok() => (p.to_string(), s.parse::<f64>().unwrap()),
                    _ => (v.clone(), 2.0),
                };
                (path, Instant::now() + Duration::from_secs_f64(secs), false)
            }),
        }
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, at, requested)) = self.screenshot.clone() else { return };
        if !requested && Instant::now() >= at {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.screenshot = Some((path.clone(), at, true));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let file = std::fs::File::create(&path).expect("screenshot file");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().expect("png header");
            wr.write_image_data(&rgba).expect("png data");
            drop(wr);
            eprintln!("screenshot saved to {path}");
            std::process::exit(0);
        }
    }

    fn input(&self, i: Input) {
        self.sim.send(Cmd::Input(i));
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    fn realtime(&self) -> bool {
        !self.status.speed_max && (self.status.speed_factor - 1.0).abs() < 1e-6
    }

    /// Combines the mouse and keyboard states of the push buttons and sends the changes.
    fn apply_buttons(&mut self) {
        for i in 0..2 {
            let pressed = self.mouse_buttons[i] || self.key_buttons[i];
            if self.buttons[i] != pressed {
                self.buttons[i] = pressed;
                self.input(Input::Button(i, pressed));
            }
        }
        for i in 0..4 {
            let pressed = self.vio_mouse[i];
            if self.vio_buttons[i] != pressed {
                self.vio_buttons[i] = pressed;
                self.input(Input::VioButton(i, pressed));
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.status = self.sim.status();
        let now = self.status.time;
        if now < self.last_time {
            // reset of the timeline (new image or power cycle)
            self.speaker.clear();
        }
        self.last_time = now;
        // display frames
        for d in 0..self.displays.len() {
            if let Some(f) = self.sim.take_frame(d) {
                let (w, h) = (f.width as usize, f.height as usize);
                if w > 0 && h > 0 && f.rgba.len() >= w * h * 4 {
                    let image = ColorImage::from_rgba_unmultiplied([w, h], &f.rgba[..w * h * 4]);
                    let disp = &mut self.displays[d];
                    let opts = if disp.nearest { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
                    match &mut disp.tex {
                        Some(t) => t.set(image.clone(), opts),
                        None => disp.tex = Some(ctx.load_texture(format!("display-{d}"), image.clone(), opts)),
                    }
                    disp.image = Some(image);
                }
            }
        }
        // camera preview
        let preview = self.sim.shared.camera_preview.lock().ok().and_then(|mut p| p.take());
        if let Some(p) = preview {
            if let Some(image) = rgba_image(&p) {
                match &mut self.cam_tex {
                    Some(t) => t.set(image, TextureOptions::LINEAR),
                    None => self.cam_tex = Some(ctx.load_texture("camera-preview", image, TextureOptions::LINEAR)),
                }
            }
        }
        // speaker: play at 1x real time only; otherwise drop the blocks to avoid drift
        let blocks = self.sim.take_speaker();
        if self.realtime() && self.status.halted.is_none() && !self.status.paused {
            for (_, rate, channels, samples) in &blocks {
                self.speaker.play(*rate, *channels, samples);
            }
        } else if !blocks.is_empty() {
            self.speaker.clear();
        }
        // terminals
        for n in 0..UARTS {
            while let Ok(data) = self.uart_rx[n].try_recv() {
                let term = &mut self.terminals[n];
                term.push_str(&String::from_utf8_lossy(&data).replace('\r', ""));
                if term.len() > 200_000 {
                    let cut = term.len() - 150_000;
                    let cut = term.char_indices().map(|(i, _)| i).find(|&i| i >= cut).unwrap_or(0);
                    term.drain(..cut);
                }
            }
        }
    }

    fn handle_files(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        for f in files {
            let path = f.path().to_path_buf();
            match cs320_loader::Image::load(&path) {
                Ok(img) => {
                    let msg = match self.sim.load(img, false) {
                        Ok(()) => format!("loaded {}", path.display()),
                        Err(e) => format!("{}: {e}", path.display()),
                    };
                    self.note(msg);
                }
                Err(e) => self.note(e.to_string()),
            }
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            self.key_buttons = [false; 2];
            return;
        }
        self.key_buttons = ctx.input(|i| [i.key_down(Key::Num1), i.key_down(Key::Num2)]);
        if ctx.input(|i| i.key_pressed(Key::R)) {
            self.input(Input::ResetButton);
        }
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.sim.send(Cmd::Pause(!self.status.paused));
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.zoom = Zoom::Fit;
        }
    }

    // ---- top bar ----

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Corstone-320 (Cortex-M85)").strong());
            ui.separator();
            let s = &self.status;
            let state = if let Some(r) = s.halted {
                format!("halted ({r:?})")
            } else if s.end_reached {
                "end of simulation".to_string()
            } else if s.paused {
                "paused".to_string()
            } else {
                "running".to_string()
            };
            let state_color =
                if s.halted.is_some() || s.paused { Color32::from_rgb(0xf0, 0xc0, 0x60) } else { Color32::from_rgb(0x80, 0xe0, 0x90) };
            ui.label(RichText::new(state).color(state_color));
            if s.debugger {
                ui.label("debugger attached");
            }
            ui.separator();
            ui.label(format!("t = {:.3} s", to_secs(s.time)));
            ui.label(format!("{:.1} MIPS", s.mips));
            ui.label(format!("{:.2}x real time", s.realtime_factor));
            ui.separator();
            ui.label(if s.secure { "Secure" } else { "Non-secure" });
            ui.separator();
            let cur = if s.speed_max { 0.0 } else { s.speed_factor };
            let mut sel = cur;
            let label = |v: f64| if v == 0.0 { "max".to_string() } else { format!("{v}x") };
            egui::ComboBox::from_id_salt("speed").selected_text(label(cur)).width(70.0).show_ui(ui, |ui| {
                for v in [0.1, 0.5, 1.0, 2.0, 10.0, 0.0] {
                    ui.selectable_value(&mut sel, v, label(v));
                }
            });
            if sel != cur {
                self.sim.send(Cmd::Speed(if sel == 0.0 { Speed::Max } else { Speed::Realtime(sel) }));
            }
            let paused = self.status.paused;
            if ui.button(if paused { "Resume" } else { "Pause" }).on_hover_text("space").clicked() {
                self.sim.send(Cmd::Pause(!paused));
            }
            if ui.button("Reset").on_hover_text("reset button (key R)").clicked() {
                self.input(Input::ResetButton);
            }
            if self.status.halted.is_some() && !self.status.debugger && ui.button("Continue").clicked() {
                self.sim.send(Cmd::Continue);
            }
        });
    }

    // ---- central display ----

    fn display_view(&mut self, ui: &mut egui::Ui) {
        let counts = self.status.frame_counts.clone();
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            for (d, name) in DISPLAY_NAMES.iter().enumerate() {
                let has = self.displays[d].tex.is_some() || counts.get(d).is_some_and(|c| *c > 0);
                if d == 0 || has {
                    ui.selectable_value(&mut self.display, d, *name);
                }
            }
            ui.separator();
            ui.label("zoom");
            ui.selectable_value(&mut self.zoom, Zoom::Fit, "fit").on_hover_text("key F");
            ui.selectable_value(&mut self.zoom, Zoom::X1, "1x");
            ui.selectable_value(&mut self.zoom, Zoom::X2, "2x");
        });
        let d = self.display.min(self.displays.len() - 1);
        let rect = ui.available_rect_before_wrap();
        let (rect, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x14, 0x16, 0x1a));
        let caption_h = 18.0;
        let view = egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - caption_h));
        let mut caption = format!("{}: no frame yet", DISPLAY_NAMES[d]);
        if let Some(image) = self.displays[d].image.as_ref() {
            let [w, h] = image.size;
            let (w, h) = (w as f32, h as f32);
            let scale = match self.zoom {
                Zoom::Fit => (view.width() / w).min(view.height() / h).max(0.01),
                Zoom::X1 => 1.0,
                Zoom::X2 => 2.0,
            };
            let nearest = (scale - scale.round()).abs() < 1e-3;
            let disp = &mut self.displays[d];
            if nearest != disp.nearest {
                disp.nearest = nearest;
                if let (Some(t), Some(img)) = (&mut disp.tex, &disp.image) {
                    t.set(img.clone(), if nearest { TextureOptions::NEAREST } else { TextureOptions::LINEAR });
                }
            }
            let size = egui::vec2(w * scale, h * scale);
            let x = if size.x <= view.width() { view.center().x - size.x / 2.0 } else { view.left() };
            let y = if size.y <= view.height() { view.center().y - size.y / 2.0 } else { view.top() };
            let img_rect = egui::Rect::from_min_size(egui::pos2(x, y).round(), size);
            if let Some(t) = &disp.tex {
                painter.image(t.id(), img_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            let frames = counts.get(d).copied().unwrap_or(0);
            let zoom_text = match self.zoom {
                Zoom::Fit => format!("fit ({scale:.2}x)"),
                Zoom::X1 => "1x".into(),
                Zoom::X2 => "2x".into(),
            };
            caption = format!("{} {}x{}   frame {}   zoom {}", DISPLAY_NAMES[d], w, h, frames, zoom_text);
        } else {
            let text = if d == 0 {
                "HDLCD: no frame yet (the display controller is not enabled)".to_string()
            } else {
                format!("{}: no frame yet", DISPLAY_NAMES[d])
            };
            painter.text(view.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(16.0), Color32::from_gray(140));
        }
        painter.text(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 3.0),
            egui::Align2::LEFT_BOTTOM,
            caption,
            egui::FontId::proportional(12.0),
            Color32::from_gray(170),
        );
        if let Some((m, t)) = &self.message {
            if t.elapsed() < Duration::from_secs(5) {
                painter.text(
                    rect.center_top() + egui::vec2(0.0, 12.0),
                    egui::Align2::CENTER_TOP,
                    m,
                    egui::FontId::proportional(15.0),
                    Color32::from_rgb(0xf0, 0xd0, 0x80),
                );
            }
        }
    }

    // ---- right panel ----

    fn controls(&mut self, ui: &mut egui::Ui) {
        self.board_io(ui);
        ui.separator();
        self.vio(ui);
        ui.separator();
        self.camera(ui);
        ui.separator();
        self.microphone(ui);
        ui.separator();
        self.speaker_ui(ui);
        let n = self.status.warnings.len();
        if n > 0 {
            ui.separator();
            ui.toggle_value(&mut self.show_warnings, format!("{n} model warning(s)"));
            if self.show_warnings {
                for w in &self.status.warnings {
                    ui.label(RichText::new(w).small().color(Color32::from_rgb(0xe0, 0xb0, 0x60)));
                }
            }
        }
        ui.separator();
        ui.label(RichText::new("Drop an .elf/.axf/.hex on the window to load it").small().weak());
    }

    fn board_io(&mut self, ui: &mut egui::Ui) {
        ui.heading("Board I/O");
        ui.horizontal(|ui| {
            for bit in (0..10).rev() {
                led(ui, self.status.leds & (1 << bit) != 0, &format!("LED {bit}"));
            }
        });
        ui.horizontal(|ui| {
            for (i, name) in ["PB1", "PB2"].iter().enumerate() {
                let resp = ui.add(egui::Button::new(*name).selected(self.buttons[i]).min_size(egui::vec2(56.0, 26.0)));
                let resp = resp.on_hover_text(format!("user button {} (key {})", i + 1, i + 1));
                self.mouse_buttons[i] = resp.is_pointer_button_down_on();
            }
            ui.label(RichText::new("hold to press").small().weak());
        });
        ui.horizontal(|ui| {
            for bit in 0..8 {
                let mut on = self.switches & (1 << bit) != 0;
                ui.vertical(|ui| {
                    ui.label(RichText::new(format!("S{bit}")).small());
                    if toggle_switch(ui, &mut on).changed() {
                        self.switches = if on { self.switches | (1 << bit) } else { self.switches & !(1 << bit) };
                        self.input(Input::Switch(bit, on));
                    }
                });
            }
        });
        let g = self.status.gpio_out;
        ui.label(
            RichText::new(format!("GPIO out  0:{:04x}  1:{:04x}  2:{:04x}  3:{:04x}", g[0].0, g[1].0, g[2].0, g[3].0)).small().monospace(),
        );
    }

    fn vio(&mut self, ui: &mut egui::Ui) {
        ui.heading("VIO");
        ui.horizontal(|ui| {
            for bit in (0..8).rev() {
                led(ui, self.status.vio_leds & (1 << bit) != 0, &format!("vioLED{bit}"));
            }
        });
        ui.horizontal(|ui| {
            for i in 0..4 {
                let resp = ui.add(egui::Button::new(format!("BTN{i}")).selected(self.vio_buttons[i]).min_size(egui::vec2(50.0, 24.0)));
                self.vio_mouse[i] = resp.on_hover_text(format!("vioBUTTON{i} (hold)")).is_pointer_button_down_on();
            }
        });
        ui.label(RichText::new("values from the host").small().weak());
        ui.horizontal(|ui| {
            for i in 0..4 {
                ui.label(RichText::new(format!("{i}:")).small());
                if ui.add(egui::DragValue::new(&mut self.vio_in[i]).speed(1.0)).changed() {
                    self.input(Input::VioValue(i, self.vio_in[i]));
                }
            }
        });
        ui.label(RichText::new("values written by the target").small().weak());
        let vals = &self.status.vio_values;
        egui::Grid::new("vio-values").num_columns(4).spacing([12.0, 2.0]).show(ui, |ui| {
            let mut shown = 0;
            for (id, v) in vals.iter().enumerate() {
                if id < 8 || *v != 0 {
                    ui.label(RichText::new(format!("[{id:2}] {v}")).monospace().small());
                    shown += 1;
                    if shown % 4 == 0 {
                        ui.end_row();
                    }
                    if shown >= 16 {
                        break;
                    }
                }
            }
        });
    }

    fn camera(&mut self, ui: &mut egui::Ui) {
        ui.heading("Camera");
        let before = self.cam_sel;
        egui::ComboBox::from_id_salt("camera-source")
            .selected_text(match self.cam_sel {
                CamSel::TestPattern => "test pattern",
                CamSel::None => "none (disconnected)",
                CamSel::Image => "image file",
                CamSel::Folder => "image folder",
                CamSel::Live => "live (webcam)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.cam_sel, CamSel::TestPattern, "test pattern");
                ui.selectable_value(&mut self.cam_sel, CamSel::None, "none (disconnected)");
                ui.selectable_value(&mut self.cam_sel, CamSel::Image, "image file");
                ui.selectable_value(&mut self.cam_sel, CamSel::Folder, "image folder");
                ui.selectable_value(&mut self.cam_sel, CamSel::Live, "live (webcam)");
            });
        if self.cam_sel != before {
            self.cam_note = None;
            self.webcam = None;
            match self.cam_sel {
                CamSel::TestPattern => self.input(Input::Camera(CameraSource::TestPattern)),
                CamSel::None => self.input(Input::Camera(CameraSource::None)),
                CamSel::Live => {
                    self.start_webcam();
                    self.input(Input::Camera(CameraSource::Live));
                }
                CamSel::Image | CamSel::Folder => self.use_camera_path(),
            }
        }
        if matches!(self.cam_sel, CamSel::Image | CamSel::Folder) {
            ui.horizontal(|ui| {
                let hint = if self.cam_sel == CamSel::Image { "path of a PNG/JPEG" } else { "folder of images" };
                let resp =
                    ui.add(egui::TextEdit::singleline(&mut self.cam_path).hint_text(hint).desired_width(ui.available_width() - 50.0));
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Use").clicked() || enter {
                    self.use_camera_path();
                }
            });
        }
        if let Some(t) = &self.cam_tex {
            let [w, h] = t.size();
            let width = ui.available_width().min(320.0);
            let size = egui::vec2(width, width * h as f32 / w.max(1) as f32);
            ui.add(egui::Image::from_texture(t).fit_to_exact_size(size));
        }
        let s = &self.status;
        let text = if let Some(e) = &s.camera_error {
            RichText::new(e).small().color(Color32::from_rgb(0xf0, 0x80, 0x80))
        } else if s.camera_connected {
            RichText::new("connected").small().color(Color32::from_rgb(0x80, 0xe0, 0x90))
        } else {
            RichText::new("not connected").small().weak()
        };
        ui.label(text);
        if let Some(n) = &self.cam_note {
            ui.label(RichText::new(n).small().color(Color32::from_rgb(0xe0, 0xb0, 0x60)));
        }
    }

    /// Starts the host webcam (device 0) feeding the camera's live slot.
    fn start_webcam(&mut self) {
        let Some(slot) = self.sim.with(|m| m.sys.board.camera.live.clone()) else { return };
        match cs320_board::sources::Webcam::start(slot, 0, 640, 480) {
            Ok(w) => self.webcam = Some(w),
            Err(e) => self.cam_note = Some(e),
        }
    }

    fn use_camera_path(&mut self) {
        let p = PathBuf::from(self.cam_path.trim());
        if self.cam_path.trim().is_empty() {
            self.cam_note = Some("enter a path and press Use".into());
            return;
        }
        // a folder is a folder whatever the combo says
        let src = if p.is_dir() {
            self.cam_sel = CamSel::Folder;
            CameraSource::Folder(p)
        } else {
            self.cam_sel = CamSel::Image;
            CameraSource::Image(p)
        };
        self.cam_note = None;
        self.input(Input::Camera(src));
    }

    fn microphone(&mut self, ui: &mut egui::Ui) {
        ui.heading("Microphone");
        let before = self.mic_sel;
        egui::ComboBox::from_id_salt("mic-source")
            .selected_text(match self.mic_sel {
                MicSel::Silence => "silence",
                MicSel::Tone => "tone",
                MicSel::Wav => "WAV file",
                MicSel::Live => "live (host input)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.mic_sel, MicSel::Silence, "silence");
                ui.selectable_value(&mut self.mic_sel, MicSel::Tone, "tone");
                ui.selectable_value(&mut self.mic_sel, MicSel::Wav, "WAV file");
                ui.selectable_value(&mut self.mic_sel, MicSel::Live, "live (host input)");
            });
        if self.mic_sel != before {
            self.mic_note = None;
            if self.mic_sel != MicSel::Live {
                self.live_mic = None;
            }
            match self.mic_sel {
                MicSel::Silence => self.input(Input::Microphone(MicSource::Silence)),
                MicSel::Tone => self.input(Input::Microphone(MicSource::Tone(self.tone_hz, self.tone_amp))),
                MicSel::Wav => self.use_wav_path(),
                MicSel::Live => self.start_live_mic(),
            }
        }
        match self.mic_sel {
            MicSel::Tone => {
                let r1 = ui.add(egui::Slider::new(&mut self.tone_hz, 50.0..=8000.0).logarithmic(true).text("Hz"));
                let r2 = ui.add(egui::Slider::new(&mut self.tone_amp, 0.0..=1.0).text("amplitude"));
                if r1.changed() || r2.changed() {
                    self.input(Input::Microphone(MicSource::Tone(self.tone_hz, self.tone_amp)));
                }
            }
            MicSel::Wav => {
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.wav_path)
                            .hint_text("path of a WAV file")
                            .desired_width(ui.available_width() - 50.0),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.button("Use").clicked() || enter {
                        self.use_wav_path();
                    }
                });
            }
            MicSel::Live => {
                if let Some(m) = &self.live_mic {
                    ui.label(RichText::new(format!("host microphone at {} Hz", m.rate)).small().weak());
                }
            }
            MicSel::Silence => {}
        }
        if ui.add(egui::Slider::new(&mut self.mic_noise, 0.0..=0.2).text("noise")).changed() {
            self.input(Input::MicNoise(self.mic_noise));
        }
        level_meter(ui, self.status.mic_level, "level");
        if let Some(e) = &self.status.mic_error {
            ui.label(RichText::new(e).small().color(Color32::from_rgb(0xf0, 0x80, 0x80)));
        }
        if let Some(n) = &self.mic_note {
            ui.label(RichText::new(n).small().color(Color32::from_rgb(0xe0, 0xb0, 0x60)));
        }
    }

    fn use_wav_path(&mut self) {
        let p = self.wav_path.trim();
        if p.is_empty() {
            self.mic_note = Some("enter a path and press Use".into());
            return;
        }
        self.mic_note = None;
        self.input(Input::Microphone(MicSource::Wav(PathBuf::from(p))));
    }

    /// Opens the host microphone and routes it into the board's live slot.
    fn start_live_mic(&mut self) {
        let Some(slot) = self.sim.with(|m| m.sys.board.mic.live.clone()) else { return };
        match audio::open_live_mic(slot) {
            Ok(m) => {
                let rate = m.rate;
                self.sim.with(move |m| m.sys.board.mic.live_rate = rate);
                self.live_mic = Some(m);
                self.input(Input::Microphone(MicSource::Live));
            }
            Err(e) => {
                self.mic_note = Some(format!("live microphone: {e}"));
                self.mic_sel = MicSel::Silence;
                self.input(Input::Microphone(MicSource::Silence));
            }
        }
    }

    fn speaker_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Speaker");
        level_meter(ui, self.status.speaker_level, "level");
        if self.speaker.available {
            ui.add(egui::Slider::new(&mut self.speaker.volume, 0.0..=1.0).text("volume"));
            ui.checkbox(&mut self.speaker.muted, "mute");
            if !self.realtime() {
                ui.label(RichText::new("playback only at 1x real time").small().weak());
            }
        } else {
            ui.label(RichText::new("no audio output").small().weak());
        }
    }

    // ---- bottom panel ----

    fn terminal(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for n in 0..UARTS {
                ui.selectable_value(&mut self.tab, n, format!("UART{n}"));
            }
            ui.separator();
            if ui.small_button("clear").clicked() {
                self.terminals[self.tab].clear();
            }
            egui::ComboBox::from_id_salt("line-end")
                .selected_text(match self.line_end {
                    LineEnd::None => "no line end",
                    LineEnd::Lf => "LF",
                    LineEnd::Cr => "CR",
                    LineEnd::CrLf => "CR LF",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.line_end, LineEnd::None, "no line end");
                    ui.selectable_value(&mut self.line_end, LineEnd::Lf, "LF");
                    ui.selectable_value(&mut self.line_end, LineEnd::Cr, "CR");
                    ui.selectable_value(&mut self.line_end, LineEnd::CrLf, "CR LF");
                });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.input_line)
                    .hint_text(format!("send to UART{}", self.tab))
                    .desired_width(ui.available_width() - 60.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Send").clicked() || enter {
                let mut text = std::mem::take(&mut self.input_line);
                text.push_str(match self.line_end {
                    LineEnd::None => "",
                    LineEnd::Lf => "\n",
                    LineEnd::Cr => "\r",
                    LineEnd::CrLf => "\r\n",
                });
                self.sim.send(Cmd::UartIn(self.tab, text.into_bytes()));
                resp.request_focus();
            }
        });
        let text = &self.terminals[self.tab];
        egui::ScrollArea::vertical().id_salt(("terminal", self.tab)).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace().size(12.5)).wrap());
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.screenshot(&ctx);
        self.handle_files(&ctx);
        self.keyboard(&ctx);
        egui::Panel::top("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::right("controls").resizable(true).default_size(340.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });
        egui::Panel::bottom("terminal").resizable(true).default_size(200.0).show(ui, |ui| self.terminal(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.display_view(ui));
        self.apply_buttons();
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

/// An egui image of a camera frame.
fn rgba_image(p: &Rgba) -> Option<ColorImage> {
    let (w, h) = (p.width as usize, p.height as usize);
    if w == 0 || h == 0 || p.data.len() < w * h * 4 {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied([w, h], &p.data[..w * h * 4]))
}

/// A round LED with its label underneath.
fn led(ui: &mut egui::Ui, lit: bool, label: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(26.0, 18.0), Sense::hover());
        let p = ui.painter();
        let c = rect.center();
        if lit {
            p.circle_filled(c, 9.0, Color32::from_rgba_unmultiplied(0x40, 0xe0, 0x50, 60));
        }
        p.circle(c, 6.5, if lit { LED_ON } else { LED_OFF }, egui::Stroke::new(1.0, Color32::from_gray(90)));
        resp.on_hover_text(label);
        let short = label.rsplit(|c: char| !c.is_ascii_digit()).next().unwrap_or(label);
        ui.label(RichText::new(short).small().weak());
    });
}

/// A horizontal level meter (0..1).
fn level_meter(ui: &mut egui::Ui, level: f32, label: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).small());
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(220.0), 12.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 2.0, Color32::from_gray(40));
        let w = rect.width() * level.clamp(0.0, 1.0);
        let color = if level > 0.9 { Color32::from_rgb(0xf0, 0x60, 0x50) } else { Color32::from_rgb(0x50, 0xc0, 0x60) };
        p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height())), 2.0, color);
        ui.label(RichText::new(format!("{:3.0} %", level * 100.0)).small().monospace());
    });
}

/// A small sliding toggle switch.
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(28.0, 15.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let visuals = ui.style().interact_selectable(&resp, *on);
        let radius = 0.5 * rect.height();
        ui.painter().rect(rect, radius, visuals.bg_fill, visuals.bg_stroke, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter().circle(egui::pos2(x, rect.center().y), 0.7 * radius, visuals.fg_stroke.color, visuals.fg_stroke);
    }
    resp
}
