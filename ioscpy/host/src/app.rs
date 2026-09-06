//! One native window: USB device picker, then the live session.
//! Launch `ioscpy.exe` — no console, no extra processes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Frame, Key, Margin, Pos2, Rect,
    Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};

use crate::auto;
use crate::clipboard;
use crate::cli::Cli;
use crate::device::{self, Device};
use crate::input::{map_to_norm, InputFrame};
use crate::protocol::{self, KeyCode, MessageType, SystemAction, TouchPhase};
use crate::sidebar::{self, Action};
use crate::video::DecodedFrame;
use crate::window::{self, FrameSlot};

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
const RAIL: f32 = 64.0;
const PANEL: f32 = 304.0;
const SEARCH_ID: &str = "device-search";
const HEADER_H: f32 = 42.0;
const MIN_VIDEO_H: f32 = 360.0;
const MAX_VIDEO_H: f32 = 1280.0;
const DEFAULT_VIDEO_H: f32 = 520.0;

struct Live {
    device: Device,
    frames: FrameSlot,
    stop: Arc<AtomicBool>,
    input_tx: Sender<InputFrame>,
    clip_rx: Receiver<String>,
    error: Arc<Mutex<Option<String>>>,
    texture: Option<TextureHandle>,
    frame_wh: (usize, usize),
    down: bool,
    last_xy: (f32, f32),
    clip: Arc<Mutex<ClipBook>>,
    last_clip: Instant,
    latin: LatinBuf,
    status: String,
    got_frame: bool,
    t0: Instant,
    auto: auto::Job,
}

#[derive(Default)]
struct ClipBook {
    last_change_count: i64,
    last_synced_hash: Option<u64>,
}

struct LatinBuf {
    text: String,
    last_input: Instant,
    last_paste: Option<Instant>,
}

impl Default for LatinBuf {
    fn default() -> Self {
        Self {
            text: String::new(),
            last_input: Instant::now(),
            last_paste: None,
        }
    }
}

enum ScanMsg {
    Ok(Vec<Device>),
    Err(String),
}

pub struct IoscpyApp {
    cli: Cli,
    sessions: Vec<Live>,
    focus: Option<String>,
    devices: Vec<Device>,
    scan_error: Option<String>,
    session_error: Option<String>,
    last_scan: Instant,
    scanning: bool,
    scan_rx: Option<Receiver<ScanMsg>>,
    icons: HashMap<u8, TextureHandle>,
    auto_udid: Option<String>,
    search: String,
    pin: String,
}

pub fn run(cli: Cli) -> Result<()> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/AppIcon.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1400.0, 820.0])
        .with_min_inner_size([960.0, 600.0])
        .with_title("ioscpy")
        .with_decorations(true);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        vsync: true,
        ..Default::default()
    };

    let auto = cli.device.clone();
    eframe::run_native(
        "ioscpy",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(IoscpyApp::new(cli, auto)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("couldn't open the ioscpy window: {e}"))
}

impl IoscpyApp {
    fn new(cli: Cli, auto_udid: Option<String>) -> Self {
        let mut app = Self {
            cli,
            sessions: Vec::new(),
            focus: None,
            devices: Vec::new(),
            scan_error: None,
            session_error: None,
            last_scan: Instant::now() - Duration::from_secs(10),
            scanning: false,
            scan_rx: None,
            icons: HashMap::new(),
            auto_udid,
            search: String::new(),
            pin: "956123".into(),
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
                if let Some(udid) = self.auto_udid.take() {
                    if let Some(dev) = self.devices.iter().find(|d| d.udid == udid).cloned() {
                        self.begin_live(dev);
                    }
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

    fn begin_live(&mut self, device: Device) {
        if self.sessions.iter().any(|s| s.device.udid == device.udid) {
            self.focus = Some(device.udid);
            return;
        }
        self.session_error = None;
        let mut cli = self.cli.clone();
        cli.device = Some(device.udid.clone());
        let port = cli.port.unwrap_or(protocol::DEFAULT_PORT);

        let stop = Arc::new(AtomicBool::new(false));
        let frames = window::new_frame_slot();
        let (input_tx, input_rx) = mpsc::channel();
        let (clip_tx, clip_rx) = mpsc::channel::<String>();
        let error = Arc::new(Mutex::new(None));

        let net_stop = stop.clone();
        let net_frames = frames.clone();
        let net_err = error.clone();
        thread::spawn(move || {
            if let Err(e) = crate::run_connection_loop(
                &cli,
                port,
                &net_stop,
                Some(net_frames),
                Some(input_rx),
                Some(clip_tx),
            ) {
                if let Ok(mut slot) = net_err.lock() {
                    *slot = Some(format!("{e:#}"));
                }
            }
            net_stop.store(true, Ordering::Relaxed);
        });

        self.focus = Some(device.udid.clone());
        self.sessions.push(Live {
            device,
            frames,
            stop,
            input_tx,
            clip_rx,
            error,
            texture: None,
            frame_wh: (0, 0),
            down: false,
            last_xy: (0.5, 0.5),
            clip: Arc::new(Mutex::new(ClipBook::default())),
            last_clip: Instant::now(),
            latin: LatinBuf::default(),
            status: "подключение…".into(),
            got_frame: false,
            t0: Instant::now(),
            auto: auto::Job::idle(),
        });
    }

    fn end_live(&mut self, udid: &str) {
        if let Some(s) = self.sessions.iter().find(|s| s.device.udid == udid) {
            s.stop.store(true, Ordering::Relaxed);
        }
        self.sessions.retain(|s| s.device.udid != udid);
        if self.focus.as_deref() == Some(udid) {
            self.focus = self.sessions.first().map(|s| s.device.udid.clone());
        }
    }

    fn is_live(&self, udid: &str) -> bool {
        self.sessions.iter().any(|s| s.device.udid == udid)
    }
}

impl eframe::App for IoscpyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.take_scan();
        if self.scanning && self.last_scan.elapsed() > Duration::from_secs(8) {
            self.scanning = false;
            self.scan_rx = None;
            self.scan_error = Some(
                "поиск устройств завис. Проверь кабель и Apple Mobile Device Support.".into(),
            );
        }
        if !self.scanning && self.last_scan.elapsed() > Duration::from_secs(4) {
            self.scan_now();
        }

        let mut dead: Vec<(String, Option<String>)> = Vec::new();
        let has_sessions = !self.sessions.is_empty();

        egui::SidePanel::left("device_panel")
            .exact_width(PANEL)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .inner_margin(0.0),
            )
            .show(ctx, |ui| draw_panel(ui, self));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(0.0))
            .show(ctx, |ui| {
                if has_sessions {
                    dead = draw_workspace(ui, ctx, self);
                } else {
                    draw_idle(ui, self);
                }
            });

        for (udid, err) in dead {
            self.end_live(&udid);
            if let Some(e) = err {
                self.session_error = Some(e);
            }
        }

        if has_sessions {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for s in &self.sessions {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn apply_style(ctx: &egui::Context) {
    apply_fonts(ctx);
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
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(10);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(10);
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    ctx.set_style(style);
}

fn apply_fonts(ctx: &egui::Context) {
    let path = if cfg!(windows) {
        r"C:\Windows\Fonts\segoeui.ttf"
    } else if cfg!(target_os = "macos") {
        "/System/Library/Fonts/SFNSText.ttf"
    } else {
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("system".into(), egui::FontData::from_owned(bytes).into());
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        fam.insert(0, "system".into());
    }
    ctx.set_fonts(fonts);
}

fn draw_panel(ui: &mut Ui, app: &mut IoscpyApp) {
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("ioscpy")
                .font(FontId::proportional(22.0))
                .color(TEXT)
                .strong(),
        );
        ui.add_space(6.0);
        let (line, _) = ui.allocate_exact_size(Vec2::new(28.0, 3.0), Sense::hover());
        ui.painter()
            .rect_filled(line, CornerRadius::same(2), ACCENT);
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("v{VERSION}"))
                .size(12.0)
                .color(MUTED),
        );
    });
    ui.add_space(16.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new("Устройства")
                .size(13.0)
                .color(MUTED)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(16.0);
            let label = if app.scanning { "…" } else { "обновить" };
            if ghost_button(ui, label).clicked() {
                app.scan_now();
            }
        });
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.add(
            egui::TextEdit::singleline(&mut app.search)
                .id(egui::Id::new(SEARCH_ID))
                .hint_text("поиск…")
                .desired_width(PANEL - 48.0)
                .margin(Margin::symmetric(10, 6)),
        );
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(egui::RichText::new("PIN").size(12.0).color(MUTED));
        ui.add(
            egui::TextEdit::singleline(&mut app.pin)
                .id(egui::Id::new("auto-pin"))
                .desired_width(88.0)
                .margin(Margin::symmetric(8, 4)),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.set_width(PANEL - 32.0);
        ui.label(
            egui::RichText::new("Авто: разблокирует, откроет Деньги, выберет Югов Р., введёт 0805.")
                .size(11.0)
                .color(MUTED),
        );
    });
    ui.add_space(12.0);

    if let Some(err) = app.scan_error.clone() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(PANEL - 32.0);
                ui.label(egui::RichText::new(err).size(12.0).color(DANGER));
            });
        });
        ui.add_space(8.0);
    }

    let q = app.search.trim().to_lowercase();
    let devices: Vec<Device> = app
        .devices
        .iter()
        .filter(|d| {
            q.is_empty()
                || d.name.to_lowercase().contains(&q)
                || d.model_label().to_lowercase().contains(&q)
                || d.udid.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(PANEL);
            if devices.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.set_width(PANEL - 32.0);
                    ui.label(
                        egui::RichText::new(if app.scanning {
                            "Ищу по USB…"
                        } else if q.is_empty() {
                            "Нет iPhone по USB"
                        } else {
                            "Ничего не найдено"
                        })
                        .size(14.0)
                        .color(TEXT)
                        .strong(),
                    );
                    if q.is_empty() && !app.scanning {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("Подключи кабель и нажми «Доверять».")
                                .size(12.0)
                                .color(MUTED),
                        );
                    }
                    if app.scanning {
                        ui.add_space(10.0);
                        ui.spinner();
                    }
                });
            } else {
                for dev in &devices {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.set_width(PANEL - 32.0);
                            device_card(ui, app, dev);
                        });
                    });
                    ui.add_space(8.0);
                }
            }
            ui.add_space(16.0);
        });
}

fn draw_idle(ui: &mut Ui, app: &IoscpyApp) {
    if let Some(err) = app.session_error.as_deref() {
        ui.add_space(16.0);
        error_banner(ui, err);
    }
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            paint_phone(ui);
            ui.add_space(18.0);
            ui.label(
                egui::RichText::new("Панель устройств")
                    .size(22.0)
                    .color(TEXT)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Выбери один или несколько iPhone слева.\nЭкраны откроются рядом — размер меняется за уголок.")
                    .size(14.0)
                    .color(MUTED),
            );
        });
    });
}

fn paint_phone(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(52.0, 92.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(12),
        Color32::from_rgb(28, 28, 34),
        Stroke::new(1.5_f32, LINE),
        egui::StrokeKind::Inside,
    );
    let screen = rect.shrink2(Vec2::new(6.0, 10.0));
    painter.rect_filled(screen, CornerRadius::same(6), Color32::from_rgb(16, 18, 28));
    painter.line_segment(
        [
            Pos2::new(rect.center().x - 8.0, rect.top() + 5.0),
            Pos2::new(rect.center().x + 8.0, rect.top() + 5.0),
        ],
        Stroke::new(2.0_f32, LINE),
    );
}

fn device_card(ui: &mut Ui, app: &mut IoscpyApp, dev: &Device) {
    let live = app.is_live(&dev.udid);
    let mut connect = false;
    let mut disconnect = false;
    let stroke = if live {
        Stroke::new(1.0_f32, ACCENT)
    } else {
        Stroke::new(1.0_f32, LINE)
    };
    let fill = if live {
        Color32::from_rgb(28, 32, 48)
    } else {
        CARD
    };
    let hover = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(14)
        .inner_margin(Margin::symmetric(12, 12))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 4.0, if live { LIVE } else { Color32::from_rgb(80, 80, 92) });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&dev.name)
                            .size(14.5)
                            .color(TEXT)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · iOS {}",
                            dev.model_label(),
                            dev.ios_version
                        ))
                        .size(11.5)
                        .color(MUTED),
                    );
                    ui.label(
                        egui::RichText::new(dev.short_udid())
                            .size(11.0)
                            .color(MUTED),
                    );
                });
            });
            ui.add_space(8.0);
            if live {
                ui.label(
                    egui::RichText::new("в эфире")
                        .size(11.0)
                        .color(LIVE)
                        .strong(),
                );
                ui.add_space(4.0);
                if ghost_button(ui, "Отключить").clicked() {
                    disconnect = true;
                }
            } else if accent_button(ui, "Подключить").clicked() {
                connect = true;
            }
        });
    if live {
        if hover.response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if hover.response.clicked() {
            app.focus = Some(dev.udid.clone());
        }
    } else if hover.response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if !live
        && ui
            .interact(hover.response.rect, ui.id().with(&dev.udid), Sense::click())
            .clicked()
    {
        connect = true;
    }
    if connect {
        app.begin_live(dev.clone());
    }
    if disconnect {
        app.end_live(&dev.udid);
    }
}

fn error_banner(ui: &mut Ui, err: &str) {
    let max_w = 640.0;
    let side = ((ui.available_width() - max_w) / 2.0).max(24.0);
    ui.horizontal(|ui| {
        ui.add_space(side);
        Frame::new()
            .fill(Color32::from_rgb(42, 18, 22))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(80, 32, 40)))
            .corner_radius(12)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width().min(max_w));
                ui.label(egui::RichText::new(err).size(13.0).color(DANGER));
            });
    });
}

fn pump_session(ctx: &egui::Context, live: &mut Live) -> Option<String> {
    let mut fatal = None;
    if let Ok(mut slot) = live.error.lock() {
        if let Some(e) = slot.take() {
            fatal = Some(e);
        }
    }
    if live.stop.load(Ordering::Relaxed) && live.t0.elapsed() > Duration::from_secs(2) && !live.got_frame
    {
        fatal = Some(
            live.error
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| "связь оборвалась".into()),
        );
    }

    while let Ok(text) = live.clip_rx.try_recv() {
        apply_remote_clipboard(&text, &live.clip);
    }
    if !live.auto.running() {
        if live.last_clip.elapsed() >= Duration::from_millis(300) {
            live.last_clip = Instant::now();
            poll_clipboard(&live.input_tx, &live.clip);
        }
        flush_latin(&live.input_tx, &mut live.latin, false);
    }

    if let Some(decoded) = live.frames.lock().ok().and_then(|mut g| g.take()) {
        live.got_frame = true;
        if !live.auto.running() {
            live.status = "в эфире".into();
        }
        live.frame_wh = (decoded.width, decoded.height);
        live.auto.tick(
            &live.input_tx,
            auto::classify(Some(&decoded)),
            Some(&decoded),
            Instant::now(),
        );
        if live.auto.running() {
            let s = live.auto.status();
            if !s.is_empty() {
                live.status = s.to_string();
            }
        }
        let image = frame_to_color(&decoded);
        let name = format!("frame-{}", live.device.udid);
        if let Some(tex) = &mut live.texture {
            tex.set(image, TextureOptions::LINEAR);
        } else {
            live.texture = Some(ctx.load_texture(name, image, TextureOptions::LINEAR));
        }
    } else if live.auto.running() {
        live.auto
            .tick(&live.input_tx, auto::classify(None), None, Instant::now());
        let s = live.auto.status();
        if !s.is_empty() {
            live.status = s.to_string();
        }
    }
    fatal
}

fn draw_workspace(ui: &mut Ui, ctx: &egui::Context, app: &mut IoscpyApp) -> Vec<(String, Option<String>)> {
    let mut dead = Vec::new();
    for live in &mut app.sessions {
        if let Some(err) = pump_session(ctx, live) {
            dead.push((live.device.udid.clone(), Some(err)));
        }
    }

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(format!(
                "{}  ·  потяни угол карточки — ширина подстроится под экран телефона",
                if app.sessions.len() == 1 {
                    "1 устройство".into()
                } else {
                    format!("{} устройства", app.sessions.len())
                }
            ))
            .size(12.5)
            .color(MUTED),
        );
    });
    ui.add_space(8.0);

    let focus = app.focus.clone();
    let mut new_focus = None;
    let mut close = None;

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(16.0, 16.0);
                ui.add_space(12.0);
                let pin = app.pin.clone();
                let icons = &mut app.icons;
                for live in &mut app.sessions {
                    let focused = focus.as_deref() == Some(live.device.udid.as_str());
                    let (want_focus, want_close) =
                        draw_phone_pane(ui, ctx, icons, live, focused, &pin);
                    if want_focus {
                        new_focus = Some(live.device.udid.clone());
                    }
                    if want_close {
                        close = Some(live.device.udid.clone());
                    }
                }
                ui.add_space(12.0);
            });
            ui.add_space(16.0);
        });

    if let Some(id) = new_focus {
        app.focus = Some(id);
    }
    if let Some(id) = close {
        dead.push((id, None));
    }

    if let Some(idx) = app
        .sessions
        .iter()
        .position(|s| Some(s.device.udid.as_str()) == app.focus.as_deref())
    {
        if !app.sessions[idx].auto.running() {
            handle_keys(ctx, &mut app.sessions[idx]);
        }
    }

    dead
}

fn phone_aspect(live: &Live) -> f32 {
    let (fw, fh) = live.frame_wh;
    if fw > 1 && fh > 1 {
        fw as f32 / fh as f32
    } else {
        9.0 / 19.5
    }
}

fn draw_phone_pane(
    ui: &mut Ui,
    ctx: &egui::Context,
    icons: &mut HashMap<u8, TextureHandle>,
    live: &mut Live,
    focused: bool,
    pin: &str,
) -> (bool, bool) {
    let mut want_focus = false;
    let mut want_close = false;
    let stroke = if focused {
        Stroke::new(1.5_f32, ACCENT)
    } else {
        Stroke::new(1.0_f32, LINE)
    };

    // Width is derived from the phone's aspect ratio. egui::Resize cannot
    // shrink: inner widgets fill available_size, then Resize expands back.
    let aspect = phone_aspect(live);
    let size_id = egui::Id::new(("phone-h-v4", live.device.udid.as_str()));
    let corner_id = size_id.with("corner");
    let mut video_h = ui
        .ctx()
        .data_mut(|d| d.get_persisted::<f32>(size_id).unwrap_or(DEFAULT_VIDEO_H));
    if let Some(resp) = ui.ctx().read_response(corner_id) {
        if resp.dragged() {
            let d = resp.drag_delta();
            video_h += d.y + d.x / aspect.max(0.2);
        }
    }
    video_h = video_h.clamp(MIN_VIDEO_H, MAX_VIDEO_H);
    ui.ctx().data_mut(|d| d.insert_persisted(size_id, video_h));

    let video_w = (video_h * aspect).floor().max(80.0);
    let pad = 8.0;
    let card = Vec2::new(
        video_w + 8.0 + RAIL + pad * 2.0,
        video_h + HEADER_H + 6.0 + pad * 2.0,
    );

    ui.vertical(|ui| {
        let (outer, _) = ui.allocate_exact_size(card, Sense::hover());
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(outer), |ui| {
            ui.vertical(|ui| {
                Frame::new()
                    .fill(SURFACE)
                    .stroke(stroke)
                    .corner_radius(14)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width());
                            draw_phone_header(ui, live, pin, &mut want_close);
                            ui.add_space(6.0);
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let (rect, resp) = ui.allocate_exact_size(
                                    Vec2::new(video_w, video_h),
                                    Sense::click_and_drag(),
                                );
                                paint_phone_in_rect(ui, live, rect, &resp);
                                if resp.clicked() {
                                    want_focus = true;
                                }
                                ui.push_id(("rail", live.device.udid.as_str()), |ui| {
                                    if draw_rail(
                                        ui,
                                        ctx,
                                        icons,
                                        &live.input_tx,
                                        &live.clip,
                                        focused,
                                    ) {
                                        want_focus = true;
                                        ui.memory_mut(|m| {
                                            if let Some(id) = m.focused() {
                                                m.surrender_focus(id);
                                            }
                                        });
                                    }
                                });
                            });
                        });
                    });
            });
        });

        let handle = Vec2::splat(16.0);
        let corner = Rect::from_min_size(outer.max - handle, handle);
        let cre = ui.interact(corner, corner_id, Sense::drag());
        paint_resize_corner(ui, corner, if cre.hovered() || cre.dragged() {
            TEXT
        } else {
            MUTED
        });
        if cre.hovered() || cre.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
        }
    });

    (want_focus, want_close)
}

fn draw_phone_header(ui: &mut Ui, live: &mut Live, pin: &str, want_close: &mut bool) {
    let height = 36.0;
    let close = 26.0;
    let auto_w = 52.0;
    ui.horizontal(|ui| {
        ui.set_height(height);
        ui.set_width(ui.available_width());
        let rest = (ui.available_width() - close - auto_w - 12.0).max(48.0);
        ui.allocate_ui_with_layout(
            Vec2::new(rest, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(
                    dot.center(),
                    4.0,
                    if live.got_frame { LIVE } else { MUTED },
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new(&live.device.name)
                            .size(13.5)
                            .color(TEXT)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(&live.status).size(11.0).color(MUTED));
                });
            },
        );
        let auto_label = if live.auto.running() { "стоп" } else { "авто" };
        if ui
            .add_sized(
                Vec2::new(auto_w, 26.0),
                egui::Button::new(egui::RichText::new(auto_label).size(12.0).color(TEXT))
                    .fill(if live.auto.running() {
                        ACCENT_DIM
                    } else {
                        Color32::from_rgb(36, 36, 42)
                    })
                    .corner_radius(8),
            )
            .on_hover_text("Разбудить, разблокировать, открыть Деньги, Югов Р., пароль 0805")
            .clicked()
        {
            if live.auto.running() {
                live.auto = auto::Job::idle();
                live.status = "в эфире".into();
            } else {
                live.auto = auto::Job::start(pin);
            }
        }
        if ui
            .add_sized(
                Vec2::splat(close),
                egui::Button::new(egui::RichText::new("×").size(16.0).color(TEXT))
                    .fill(Color32::from_rgb(36, 36, 42))
                    .corner_radius(8),
            )
            .clicked()
        {
            *want_close = true;
        }
    });
}

fn paint_phone_in_rect(ui: &mut Ui, live: &mut Live, rect: Rect, resp: &egui::Response) {
    ui.painter()
        .rect_filled(rect, CornerRadius::same(12), Color32::from_rgb(6, 6, 8));

    if let Some(tex) = &live.texture {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        handle_pointer(live, resp, rect);
        if resp.clicked() || resp.dragged() {
            resp.request_focus();
        }
        return;
    }

    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "ждём кадр…",
        FontId::proportional(14.0),
        MUTED,
    );
}

fn paint_resize_corner(ui: &Ui, rect: Rect, color: Color32) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.0_f32, color);
    let p = rect.right_bottom() - Vec2::splat(3.0);
    for i in 0..3 {
        let t = 4.0 + i as f32 * 4.0;
        painter.line_segment([Pos2::new(p.x - t, p.y), Pos2::new(p.x, p.y - t)], stroke);
    }
}

fn handle_pointer(live: &mut Live, resp: &egui::Response, rect: Rect) {
    let pos = resp.interact_pointer_pos();
    let pressed = resp.is_pointer_button_down_on();
    if let Some(p) = pos {
        let nx = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let (nx, ny) = map_to_norm(
            nx * rect.width(),
            ny * rect.height(),
            rect.width() as usize,
            rect.height() as usize,
            live.frame_wh.0.max(1),
            live.frame_wh.1.max(1),
        );
        live.last_xy = (nx, ny);
        if pressed && !live.down {
            send_touch(&live.input_tx, TouchPhase::Down, nx, ny);
            live.down = true;
        } else if pressed && live.down {
            send_touch(&live.input_tx, TouchPhase::Move, nx, ny);
        }
    }
    if live.down && !pressed {
        let (x, y) = live.last_xy;
        send_touch(&live.input_tx, TouchPhase::Up, x, y);
        live.down = false;
    }
}

fn draw_rail(
    ui: &mut Ui,
    ctx: &egui::Context,
    icons: &mut HashMap<u8, TextureHandle>,
    input_tx: &Sender<InputFrame>,
    clip: &Arc<Mutex<ClipBook>>,
    focused: bool,
) -> bool {
    let mut used = false;
    let stroke = if focused {
        Stroke::new(1.5_f32, ACCENT)
    } else {
        Stroke::new(1.0_f32, LINE)
    };
    ui.vertical(|ui| {
        Frame::new()
            .fill(SURFACE)
            .stroke(stroke)
            .corner_radius(14)
            .inner_margin(Margin::symmetric(8, 10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(RAIL - 16.0);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for action in sidebar::BUTTONS {
                        if rail_icon(ui, ctx, icons, action).clicked() {
                            dispatch_action(input_tx, clip, action);
                            used = true;
                        }
                    }
                });
            });
    });
    used
}

fn rail_icon(
    ui: &mut Ui,
    ctx: &egui::Context,
    icons: &mut HashMap<u8, TextureHandle>,
    action: Action,
) -> egui::Response {
    let key = action as u8;
    let tex = icons.entry(key).or_insert_with(|| {
        let (w, h, rgba) = sidebar::icon_rgba(action);
        let img = ColorImage::from_rgba_unmultiplied([w, h], rgba);
        ctx.load_texture(format!("icon-{key}"), img, TextureOptions::LINEAR)
    });
    let size = Vec2::splat(30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if resp.is_pointer_button_down_on() {
        ACCENT_DIM
    } else if resp.hovered() {
        Color32::from_rgb(36, 36, 44)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), bg);
    let img_rect = Rect::from_center_size(rect.center(), Vec2::splat(18.0));
    ui.painter().image(
        tex.id(),
        img_rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::from_rgb(230, 230, 235),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp.on_hover_text(action_tip(action))
}

fn action_tip(action: Action) -> &'static str {
    match action {
        Action::Home => "Домой",
        Action::Lock => "Блокировка",
        Action::AppSwitcher => "Переключение приложений",
        Action::Rotate => "Поворот",
        Action::Back => "Назад",
        Action::SelectAll => "Выделить всё",
        Action::Copy => "Копировать",
        Action::Paste => "Вставить",
        Action::Cut => "Вырезать",
        Action::Undo => "Отменить",
    }
}

fn search_has_focus(ctx: &egui::Context) -> bool {
    ctx.memory(|m| {
        matches!(
            m.focused(),
            Some(id) if id == egui::Id::new(SEARCH_ID) || id == egui::Id::new("auto-pin")
        )
    })
}

fn handle_keys(ctx: &egui::Context, live: &mut Live) {
    // egui-winit turns Ctrl+C/V/X into Copy/Cut/Paste and does not also
    // emit Event::Key. Skip only when the sidebar search box is focused —
    // wants_keyboard_input() is true for ANY focused widget, including the
    // phone screen after a click, and that was swallowing all typing.
    let search_focused = search_has_focus(ctx);
    let mut typed = String::new();
    ctx.input(|i| {
        let ctrl = i.modifiers.command || i.modifiers.ctrl;
        for ev in &i.events {
            match ev {
                egui::Event::Copy if !search_focused => {
                    send_key(&live.input_tx, KeyCode::Copy);
                }
                egui::Event::Cut if !search_focused => {
                    send_key(&live.input_tx, KeyCode::Cut);
                }
                egui::Event::Paste(s) if !search_focused => {
                    paste_text(&live.input_tx, &live.clip, s);
                }
                egui::Event::Text(s) if !ctrl && !search_focused => typed.push_str(s),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !search_focused => {
                    if modifiers.command || modifiers.ctrl {
                        match key {
                            Key::A => send_key(&live.input_tx, KeyCode::SelectAll),
                            Key::C => send_key(&live.input_tx, KeyCode::Copy),
                            Key::V => paste_now(&live.input_tx, &live.clip),
                            Key::X => send_key(&live.input_tx, KeyCode::Cut),
                            Key::Z => send_key(&live.input_tx, KeyCode::Undo),
                            Key::J => send_action(&live.input_tx, SystemAction::Home),
                            Key::L => send_action(&live.input_tx, SystemAction::Lock),
                            Key::T => send_action(&live.input_tx, SystemAction::AppSwitcher),
                            Key::R => send_action(&live.input_tx, SystemAction::RotateLeft),
                            _ => {}
                        }
                        continue;
                    }
                    match key {
                        Key::Enter => send_key(&live.input_tx, KeyCode::Enter),
                        Key::Tab => send_key(&live.input_tx, KeyCode::Tab),
                        Key::Escape => send_action(&live.input_tx, SystemAction::Back),
                        Key::ArrowLeft => send_key(&live.input_tx, KeyCode::Left),
                        Key::ArrowRight => send_key(&live.input_tx, KeyCode::Right),
                        Key::ArrowUp => send_key(&live.input_tx, KeyCode::Up),
                        Key::ArrowDown => send_key(&live.input_tx, KeyCode::Down),
                        Key::Backspace => {
                            if !live.latin.text.is_empty() {
                                live.latin.text.pop();
                                live.latin.last_input = Instant::now();
                            } else {
                                send_key(&live.input_tx, KeyCode::Backspace);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    });

    if typed.is_empty() {
        return;
    }

    #[cfg(windows)]
    {
        if crate::keyboard::layout_is_cyrillic() {
            flush_latin(&live.input_tx, &mut live.latin, true);
            send_typed_text(&live.input_tx, &typed);
            return;
        }
        live.latin.text.push_str(&typed);
        live.latin.last_input = Instant::now();
    }
    #[cfg(not(windows))]
    send_typed_text(&live.input_tx, &typed);
}

fn ghost_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(13.0).color(TEXT))
            .fill(Color32::from_rgb(32, 32, 38))
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(10)
            .min_size(Vec2::new(0.0, 32.0)),
    )
}

fn accent_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.5)
                .color(Color32::WHITE)
                .strong(),
        )
        .fill(ACCENT)
        .corner_radius(10)
        .min_size(Vec2::new(108.0, 36.0)),
    )
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

fn send_action(tx: &Sender<InputFrame>, action: SystemAction) {
    let _ = tx.send(InputFrame::new(
        MessageType::SystemAction,
        protocol::encode_system_action(action),
    ));
}

fn send_key(tx: &Sender<InputFrame>, code: KeyCode) {
    let _ = tx.send(InputFrame::new(
        MessageType::InputKey,
        protocol::encode_key(code),
    ));
}

fn send_typed_text(tx: &Sender<InputFrame>, text: &str) {
    if text.is_empty() {
        return;
    }
    let payload = crate::keyboard::to_hid_typeable_string(text);
    if payload.is_empty() {
        return;
    }
    let _ = tx.send(InputFrame::new(
        MessageType::InputText,
        protocol::encode_text(&payload),
    ));
}

fn send_touch(tx: &Sender<InputFrame>, phase: TouchPhase, x: f32, y: f32) {
    let _ = tx.send(InputFrame::new(
        MessageType::InputTouch,
        protocol::encode_touch(phase, 0, x, y),
    ));
}

fn send_clipboard_set(tx: &Sender<InputFrame>, text: &str, paste: bool) {
    let mut p = Vec::with_capacity(1 + text.len());
    p.push(if paste { 0x01 } else { 0x00 });
    p.extend_from_slice(text.as_bytes());
    let _ = tx.send(InputFrame::new(MessageType::ClipboardSet, p));
}

fn paste_text(tx: &Sender<InputFrame>, clip: &Arc<Mutex<ClipBook>>, text: &str) {
    if text.is_empty() || text.len() > clipboard::MAX_CLIPBOARD_BYTES {
        send_key(tx, KeyCode::Paste);
        return;
    }
    if let Ok(mut st) = clip.lock() {
        st.last_synced_hash = Some(clipboard::hash_text(text));
    }
    send_clipboard_set(tx, text, true);
}

fn paste_now(tx: &Sender<InputFrame>, clip: &Arc<Mutex<ClipBook>>) {
    match clipboard::read_text() {
        Some(text) => paste_text(tx, clip, &text),
        None => send_key(tx, KeyCode::Paste),
    }
}

fn dispatch_action(tx: &Sender<InputFrame>, clip: &Arc<Mutex<ClipBook>>, action: Action) {
    match action {
        Action::Home => send_action(tx, SystemAction::Home),
        Action::Lock => send_action(tx, SystemAction::Lock),
        Action::AppSwitcher => send_action(tx, SystemAction::AppSwitcher),
        Action::Rotate => send_action(tx, SystemAction::RotateLeft),
        Action::Back => send_action(tx, SystemAction::Back),
        Action::SelectAll => send_key(tx, KeyCode::SelectAll),
        Action::Copy => send_key(tx, KeyCode::Copy),
        Action::Paste => paste_now(tx, clip),
        Action::Cut => send_key(tx, KeyCode::Cut),
        Action::Undo => send_key(tx, KeyCode::Undo),
    }
}

fn apply_remote_clipboard(text: &str, clip: &Arc<Mutex<ClipBook>>) {
    if text.is_empty() {
        return;
    }
    let h = clipboard::hash_text(text);
    let mut st = clip.lock().unwrap();
    if st.last_synced_hash == Some(h) {
        return;
    }
    st.last_synced_hash = Some(h);
    st.last_change_count = clipboard::write_text(text);
}

fn poll_clipboard(tx: &Sender<InputFrame>, clip: &Arc<Mutex<ClipBook>>) {
    let cc = clipboard::change_count();
    {
        let mut st = clip.lock().unwrap();
        if cc == st.last_change_count {
            return;
        }
        st.last_change_count = cc;
    }
    let text = match clipboard::read_text() {
        Some(t) if !t.is_empty() && t.len() <= clipboard::MAX_CLIPBOARD_BYTES => t,
        _ => return,
    };
    let h = clipboard::hash_text(&text);
    {
        let mut st = clip.lock().unwrap();
        if st.last_synced_hash == Some(h) {
            return;
        }
        st.last_synced_hash = Some(h);
    }
    send_clipboard_set(tx, &text, false);
}

fn flush_latin(tx: &Sender<InputFrame>, buf: &mut LatinBuf, force: bool) {
    if buf.text.is_empty() {
        return;
    }
    if !force {
        if buf.last_input.elapsed() < Duration::from_millis(70) {
            return;
        }
        if buf.last_paste.is_some_and(|t| t.elapsed() < Duration::from_millis(90)) {
            return;
        }
    }
    let mut wrapped = String::with_capacity(buf.text.len() + 6);
    wrapped.push('\u{2060}');
    wrapped.push_str(&buf.text);
    wrapped.push('\u{2060}');
    send_clipboard_set(tx, &wrapped, true);
    buf.last_paste = Some(Instant::now());
    buf.text.clear();
}
