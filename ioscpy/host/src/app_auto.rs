//! Parallel host for UI-tree automation. Does not replace the main ioscpy window.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontId, Frame, Margin, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};

use crate::auto_api::{self, Dump, Job};
use crate::cli::Cli;
use crate::device::{self, Device};
use crate::input::{map_to_norm, InputFrame};
use crate::protocol::{self, MessageType, TouchPhase};
use crate::video::DecodedFrame;
use crate::window::{self, FrameSlot};

const BG: Color32 = Color32::from_rgb(10, 10, 12);
const SURFACE: Color32 = Color32::from_rgb(18, 18, 21);
const CARD: Color32 = Color32::from_rgb(24, 24, 28);
const LINE: Color32 = Color32::from_rgb(42, 42, 50);
const TEXT: Color32 = Color32::from_rgb(244, 244, 247);
const MUTED: Color32 = Color32::from_rgb(142, 142, 154);
const ACCENT: Color32 = Color32::from_rgb(124, 156, 255);
const ACCENT_DIM: Color32 = Color32::from_rgb(70, 92, 168);
const LIVE: Color32 = Color32::from_rgb(52, 211, 153);
const DANGER: Color32 = Color32::from_rgb(248, 113, 113);
const PANEL: f32 = 320.0;

struct Live {
    device: Device,
    frames: FrameSlot,
    stop: Arc<AtomicBool>,
    input_tx: Sender<InputFrame>,
    ui_rx: Receiver<(MessageType, Vec<u8>)>,
    error: Arc<Mutex<Option<String>>>,
    texture: Option<TextureHandle>,
    frame_wh: (usize, usize),
    down: bool,
    last_xy: (f32, f32),
    status: String,
    diag: Arc<Mutex<Vec<String>>>,
    got_frame: bool,
    auto: Job,
    dump: Option<Dump>,
    profiles: Vec<String>,
}

enum ScanMsg {
    Ok(Vec<Device>),
    Err(String),
}

pub struct AutoApp {
    cli: Cli,
    live: Option<Live>,
    devices: Vec<Device>,
    selected: Option<String>,
    scan_error: Option<String>,
    session_error: Option<String>,
    scanning: bool,
    scan_rx: Option<Receiver<ScanMsg>>,
    last_scan: Instant,
    lock_pin: String,
    app_pin: String,
    profile: String,
    app_name: String,
}

pub fn run(cli: Cli) -> Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/AppIcon.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([960.0, 580.0])
        .with_title("ioscpy auto")
        .with_decorations(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "ioscpy-auto",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(AutoApp::new(cli)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("couldn't open ioscpy auto: {e}"))
}

impl AutoApp {
    fn new(cli: Cli) -> Self {
        let mut app = Self {
            selected: cli.device.clone(),
            cli,
            live: None,
            devices: Vec::new(),
            scan_error: None,
            session_error: None,
            scanning: false,
            scan_rx: None,
            last_scan: Instant::now() - Duration::from_secs(10),
            lock_pin: auto_api::LOCK_PIN_DEFAULT.into(),
            app_pin: auto_api::APP_PIN_DEFAULT.into(),
            profile: auto_api::PROFILE_DEFAULT.into(),
            app_name: auto_api::APP_DEFAULT.into(),
        };
        app.scan_now();
        app
    }

    fn scan_now(&mut self) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        self.last_scan = Instant::now();
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        thread::spawn(move || {
            let msg = match device::list_devices() {
                Ok(list) => ScanMsg::Ok(list),
                Err(e) => ScanMsg::Err(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn take_scan(&mut self) {
        let Some(rx) = &self.scan_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(ScanMsg::Ok(list)) => {
                self.devices = list;
                self.scan_error = None;
                self.scanning = false;
                self.scan_rx = None;
                if self.selected.is_none() {
                    self.selected = self.devices.first().map(|d| d.udid.clone());
                }
            }
            Ok(ScanMsg::Err(e)) => {
                self.scan_error = Some(e);
                self.scanning = false;
                self.scan_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.scanning = false;
                self.scan_rx = None;
            }
        }
    }

    fn connect(&mut self) {
        let Some(udid) = self.selected.clone() else {
            self.session_error = Some("выбери телефон слева".into());
            return;
        };
        let Some(device) = self.devices.iter().find(|d| d.udid == udid).cloned() else {
            self.session_error = Some("телефон не найден, обнови список".into());
            return;
        };
        self.disconnect();
        self.session_error = None;
        let mut cli = self.cli.clone();
        cli.device = Some(device.udid.clone());
        let port = cli.port.unwrap_or(protocol::DEFAULT_PORT);
        let stop = Arc::new(AtomicBool::new(false));
        let frames = window::new_frame_slot();
        let (input_tx, input_rx) = mpsc::channel();
        let (ui_tx, ui_rx) = mpsc::channel();
        let error = Arc::new(Mutex::new(None));
        let diag = Arc::new(Mutex::new(Vec::<String>::new()));
        let net_stop = stop.clone();
        let net_frames = frames.clone();
        let net_err = error.clone();
        let net_diag = diag.clone();
        thread::spawn(move || {
            if let Err(e) = crate::run_connection_loop(
                &cli,
                port,
                &net_stop,
                Some(net_frames),
                Some(input_rx),
                Some(ui_tx),
                Some(net_err.clone()),
                Some(net_diag),
            ) {
                if let Ok(mut slot) = net_err.lock() {
                    *slot = Some(format!("{e:#}"));
                }
            }
            net_stop.store(true, Ordering::Relaxed);
        });
        self.live = Some(Live {
            device,
            frames,
            stop,
            input_tx,
            ui_rx,
            error,
            diag,
            texture: None,
            frame_wh: (0, 0),
            down: false,
            last_xy: (0.5, 0.5),
            status: "подключение…".into(),
            got_frame: false,
            auto: Job::idle(),
            dump: None,
            profiles: Vec::new(),
        });
    }

    fn disconnect(&mut self) {
        if let Some(live) = &self.live {
            live.stop.store(true, Ordering::Relaxed);
        }
        self.live = None;
    }
}

impl eframe::App for AutoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));
        self.take_scan();
        if self.last_scan.elapsed() > Duration::from_secs(8) && !self.scanning {
            self.scan_now();
        }
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BG))
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    draw_panel(ui, self);
                    if let Some(live) = self.live.as_mut() {
                        pump_live(ctx, live, &mut self.profile);
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            draw_preview(ui, live);
                        });
                    } else {
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(120.0);
                            ui.label(
                                egui::RichText::new("Выбери телефон и нажми подключить")
                                    .size(16.0)
                                    .color(MUTED),
                            );
                        });
                    }
                });
            });
    }
}

fn pump_live(ctx: &egui::Context, live: &mut Live, profile: &mut String) {
    if let Ok(mut slot) = live.error.lock() {
        if let Some(e) = slot.take() {
            live.status = e;
        }
    }
    while let Ok((kind, payload)) = live.ui_rx.try_recv() {
        if kind == MessageType::Error {
            if let Ok(e) = serde_json::from_slice::<crate::protocol::DaemonError>(&payload) {
                live.status = e.message;
            }
        } else if kind == MessageType::UiDumpResult {
            if let Some(dump) = Dump::parse(&payload) {
                let profiles = dump.profiles();
                if !profiles.is_empty() {
                    live.profiles = profiles;
                    if profile.is_empty() && !live.profiles.is_empty() {
                        *profile = live.profiles[0].clone();
                    }
                }
                live.auto.dump_arrived();
                live.dump = Some(dump);
            }
        }
    }
    if let Some(decoded) = live.frames.lock().ok().and_then(|mut g| g.take()) {
        live.frame_wh = (decoded.width, decoded.height);
        live.got_frame = true;
        let img = frame_to_color(&decoded);
        if let Some(tex) = &mut live.texture {
            tex.set(img, TextureOptions::LINEAR);
        } else {
            live.texture = Some(ctx.load_texture("auto-preview", img, TextureOptions::LINEAR));
        }
        if live.status == "подключение…"
            || live.status.contains("ждём кадр")
            || live.status.contains("кадр из демона")
        {
            live.status = "в эфире".into();
        }
    }
    live.auto.tick(&live.input_tx, live.dump.as_ref(), Instant::now());
    if live.auto.running() || live.auto.status().starts_with("авто:") || live.auto.status() == "готово"
    {
        live.status = live.auto.status().to_string();
    }
}

fn draw_panel(ui: &mut Ui, app: &mut AutoApp) {
    ui.allocate_ui_with_layout(
        Vec2::new(PANEL, ui.available_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0_f32, LINE))
                .inner_margin(Margin::same(16))
                .corner_radius(16)
                .show(ui, |ui| {
                    ui.set_width(PANEL - 24.0);
                    ui.label(
                        egui::RichText::new("ioscpy auto")
                            .font(FontId::proportional(20.0))
                            .color(TEXT)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Управление по дереву UI, не по картинке")
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Телефон").size(12.0).color(MUTED).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ghost(ui, if app.scanning { "…" } else { "обновить" }).clicked() {
                                app.scan_now();
                            }
                        });
                    });
                    ui.add_space(6.0);
                    if let Some(err) = &app.scan_error {
                        ui.label(egui::RichText::new(err).size(11.0).color(DANGER));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for d in &app.devices {
                                let on = app.selected.as_deref() == Some(&d.udid);
                                let label = format!("{}  ·  {}", d.name, d.model_label());
                                if ui
                                    .selectable_label(on, egui::RichText::new(label).size(13.0).color(TEXT))
                                    .clicked()
                                {
                                    app.selected = Some(d.udid.clone());
                                }
                                ui.label(egui::RichText::new(&d.udid).size(10.0).color(MUTED));
                                ui.add_space(4.0);
                            }
                            if app.devices.is_empty() && !app.scanning {
                                ui.label(egui::RichText::new("нет устройств").size(12.0).color(MUTED));
                            }
                        });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if app.live.is_some() {
                            if accent(ui, "отключить").clicked() {
                                app.disconnect();
                            }
                        } else if accent(ui, "подключить").clicked() {
                            app.connect();
                        }
                    });
                    ui.add_space(16.0);
                    field(ui, "Профиль", &mut app.profile, "Югов Р.");
                    let profiles = app
                        .live
                        .as_ref()
                        .map(|l| l.profiles.clone())
                        .unwrap_or_default();
                    if !profiles.is_empty() {
                        egui::ComboBox::from_id_salt("profiles")
                            .selected_text(if app.profile.is_empty() {
                                "выбрать из дампа".to_string()
                            } else {
                                app.profile.clone()
                            })
                            .show_ui(ui, |ui| {
                                for p in &profiles {
                                    ui.selectable_value(&mut app.profile, p.clone(), p);
                                }
                            });
                        ui.add_space(6.0);
                    }
                    field(ui, "Приложение", &mut app.app_name, "Деньги");
                    field(ui, "PIN экрана", &mut app.lock_pin, "956123");
                    field(ui, "PIN приложения", &mut app.app_pin, "0805");
                    ui.add_space(10.0);
                    if let Some(live) = app.live.as_mut() {
                        ui.horizontal(|ui| {
                            let auto_l = if live.auto.running() { "стоп" } else { "авто" };
                            if accent(ui, auto_l).clicked() {
                                if live.auto.running() {
                                    live.auto = Job::idle();
                                    live.status = "в эфире".into();
                                } else {
                                    live.dump = None;
                                    live.auto = Job::start(
                                        &app.lock_pin,
                                        &app.app_pin,
                                        &app.profile,
                                        &app.app_name,
                                    );
                                }
                            }
                            if ghost(ui, "считать UI").clicked() {
                                let _ = live
                                    .input_tx
                                    .send(InputFrame::new(MessageType::UiDump, vec![]));
                                live.status = "читаю UI…".into();
                            }
                        });
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(&live.status).size(12.0).color(
                                if live.status.starts_with("авто:") {
                                    DANGER
                                } else {
                                    MUTED
                                },
                            ),
                        );
                        let diag_lines: Vec<String> =
                            live.diag.lock().map(|g| g.clone()).unwrap_or_default();
                        if !diag_lines.is_empty() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("диагностика").size(11.0).color(MUTED),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("копировать")
                                                        .size(11.0)
                                                        .color(ACCENT),
                                                )
                                                .fill(Color32::TRANSPARENT)
                                                .stroke(Stroke::NONE),
                                            )
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(diag_lines.join("\n"));
                                            live.status = "диагностика скопирована".into();
                                        }
                                    },
                                );
                            });
                            let text = diag_lines.join("\n");
                            egui::ScrollArea::vertical()
                                .id_salt("diag")
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    let mut buf = text;
                                    ui.add(
                                        egui::TextEdit::multiline(&mut buf)
                                            .font(FontId::monospace(11.0))
                                            .desired_width(f32::INFINITY)
                                            .text_color(TEXT)
                                            .frame(false),
                                    );
                                });
                        }
                    }
                    if let Some(err) = &app.session_error {
                        ui.label(egui::RichText::new(err).size(12.0).color(DANGER));
                    }
                });
        },
    );
}

fn draw_preview(ui: &mut Ui, live: &mut Live) {
    ui.label(
        egui::RichText::new(format!("{}  ·  {}", live.device.name, live.device.model_label()))
            .size(14.0)
            .color(TEXT)
            .strong(),
    );
    ui.add_space(8.0);
    let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
    ui.painter()
        .circle_filled(dot.center(), 4.0, if live.got_frame { LIVE } else { MUTED });
    let Some(tex) = &live.texture else {
        ui.add_space(40.0);
        ui.label(egui::RichText::new("ждём кадр…").color(MUTED));
        return;
    };
    let (fw, fh) = live.frame_wh;
    if fw == 0 || fh == 0 {
        return;
    }
    let avail = ui.available_size();
    let max_h = (avail.y - 8.0).max(200.0);
    let max_w = (avail.x - 8.0).max(200.0);
    let scale = (max_w / fw as f32).min(max_h / fh as f32);
    let size = Vec2::new(fw as f32 * scale, fh as f32 * scale);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    egui::Image::new((tex.id(), size)).paint_at(ui, rect);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0_f32, LINE),
        egui::StrokeKind::Outside,
    );
    if resp.drag_started() || (resp.clicked() && !live.down) {
        let p = resp.interact_pointer_pos().unwrap_or(rect.center());
        let (nx, ny) = map_to_norm(
            p.x - rect.min.x,
            p.y - rect.min.y,
            rect.width() as usize,
            rect.height() as usize,
            fw,
            fh,
        );
        live.down = true;
        live.last_xy = (nx, ny);
        send_touch(&live.input_tx, TouchPhase::Down, nx, ny);
    } else if live.down && resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let (nx, ny) = map_to_norm(
                p.x - rect.min.x,
                p.y - rect.min.y,
                rect.width() as usize,
                rect.height() as usize,
                fw,
                fh,
            );
            live.last_xy = (nx, ny);
            send_touch(&live.input_tx, TouchPhase::Move, nx, ny);
        }
    }
    if live.down && resp.drag_stopped() {
        send_touch(&live.input_tx, TouchPhase::Up, live.last_xy.0, live.last_xy.1);
        live.down = false;
    }
}

fn field(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(egui::RichText::new(label).size(11.0).color(MUTED));
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(ui.available_width())
            .margin(Margin::symmetric(8, 6)),
    );
    ui.add_space(8.0);
}

fn accent(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(TEXT))
            .fill(ACCENT_DIM)
            .corner_radius(8)
            .min_size(Vec2::new(108.0, 32.0)),
    )
}

fn ghost(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(12.0).color(TEXT))
            .fill(Color32::from_rgb(36, 36, 42))
            .corner_radius(8),
    )
}

fn apply_style(ctx: &egui::Context) {
    let path = if cfg!(windows) {
        r"C:\Windows\Fonts\segoeui.ttf"
    } else {
        return;
    };
    if let Ok(bytes) = std::fs::read(path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("system".into(), egui::FontData::from_owned(bytes).into());
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            fam.insert(0, "system".into());
        }
        ctx.set_fonts(fonts);
    }
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = SURFACE;
    style.visuals.extreme_bg_color = CARD;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(32, 32, 38);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(42, 42, 52);
    style.visuals.widgets.active.bg_fill = ACCENT_DIM;
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(10);
    ctx.set_style(style);
}

fn frame_to_color(frame: &DecodedFrame) -> ColorImage {
    let mut rgba = vec![0u8; frame.width * frame.height * 4];
    for (i, px) in frame.buf.iter().enumerate() {
        let o = i * 4;
        rgba[o] = ((px >> 16) & 0xff) as u8;
        rgba[o + 1] = ((px >> 8) & 0xff) as u8;
        rgba[o + 2] = (px & 0xff) as u8;
        rgba[o + 3] = 255;
    }
    ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &rgba)
}

fn send_touch(tx: &Sender<InputFrame>, phase: TouchPhase, x: f32, y: f32) {
    let _ = tx.send(InputFrame::new(
        MessageType::InputTouch,
        protocol::encode_touch(phase, 0, x, y),
    ));
}
