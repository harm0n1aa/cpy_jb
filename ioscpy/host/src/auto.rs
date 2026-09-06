//! Screen-aware UI automation on a live session.
//!
//! Classifies the current frame (asleep / lock / passcode / home / in-app)
//! and drives touches + system actions toward a goal. Coordinates are
//! normalized [0, 1] like the rest of the input path.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::input::InputFrame;
use crate::protocol::{self, MessageType, SystemAction, TouchPhase};
use crate::video::DecodedFrame;

const PIN_DEFAULT: &str = "956123";
const APP_PIN: &str = "0805";
/// Third of four dock icons (black/red, badge).
const DOCK_APP: (f32, f32) = (0.625, 0.915);
const ALERT_OK: (f32, f32) = (0.50, 0.551);

/// Fallback iOS passcode pad (notched iPhone). Index = digit 0..=9.
const DEFAULT_PAD: [(f32, f32); 10] = [
    (0.50, 0.785),
    (0.25, 0.470),
    (0.50, 0.470),
    (0.75, 0.470),
    (0.25, 0.575),
    (0.50, 0.575),
    (0.75, 0.575),
    (0.25, 0.680),
    (0.50, 0.680),
    (0.75, 0.680),
];

/// Alfa in-app PIN pad — lower than the iOS lock keypad.
const ALFA_PAD: [(f32, f32); 10] = [
    (0.50, 0.805),
    (0.22, 0.505),
    (0.50, 0.505),
    (0.78, 0.505),
    (0.22, 0.605),
    (0.50, 0.605),
    (0.78, 0.605),
    (0.22, 0.705),
    (0.50, 0.705),
    (0.78, 0.705),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scene {
    NoFrame,
    Asleep,
    Lock,
    Passcode,
    Home,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Drive,
    TapApp,
    PickPerson,
    TypeAppPin,
    WaitResult,
    Done,
    Failed,
}

enum Gesture {
    Swipe {
        pts: Vec<(f32, f32)>,
        i: usize,
        next: Instant,
        then_pin: bool,
    },
    Tap {
        x: f32,
        y: f32,
        release: bool,
        next: Instant,
    },
    Pin {
        taps: Vec<(f32, f32)>,
        i: usize,
        release: bool,
        next: Instant,
    },
}

pub struct Job {
    pin: String,
    phase: Phase,
    gesture: Option<Gesture>,
    until: Instant,
    started: Instant,
    cycles: u8,
    last_scene: Scene,
    status: String,
    /// Set after waking / swiping the lock screen. Bank PIN pads must not
    /// trigger typing unless we are in this unlock path.
    allow_pin: bool,
    pin_sent: bool,
    app_pin_tries: u8,
    stall: u8,
    pad: [(f32, f32); 10],
    hits: Vec<crate::ocr::Hit>,
    ocr_pending: bool,
    last_ocr: Instant,
}

impl Default for Job {
    fn default() -> Self {
        Self::idle()
    }
}

impl Job {
    pub fn idle() -> Self {
        Self {
            pin: PIN_DEFAULT.into(),
            phase: Phase::Done,
            gesture: None,
            until: Instant::now(),
            started: Instant::now(),
            cycles: 0,
            last_scene: Scene::NoFrame,
            status: String::new(),
            allow_pin: false,
            pin_sent: false,
            app_pin_tries: 0,
            stall: 0,
            pad: DEFAULT_PAD,
            hits: Vec::new(),
            ocr_pending: false,
            last_ocr: Instant::now() - Duration::from_secs(30),
        }
    }

    pub fn start(pin: &str) -> Self {
        let pin = pin
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>();
        let pin = if pin.is_empty() {
            PIN_DEFAULT.to_string()
        } else {
            pin
        };
        Self {
            pin,
            phase: Phase::Drive,
            gesture: None,
            until: Instant::now(),
            started: Instant::now(),
            cycles: 0,
            last_scene: Scene::NoFrame,
            status: "смотрю экран…".into(),
            allow_pin: false,
            pin_sent: false,
            app_pin_tries: 0,
            stall: 0,
            pad: DEFAULT_PAD,
            hits: Vec::new(),
            ocr_pending: false,
            last_ocr: Instant::now() - Duration::from_secs(30),
        }
    }

    pub fn running(&self) -> bool {
        self.gesture.is_some()
            || matches!(
                self.phase,
                Phase::Drive
                    | Phase::TapApp
                    | Phase::PickPerson
                    | Phase::TypeAppPin
                    | Phase::WaitResult
            )
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn tick(
        &mut self,
        tx: &Sender<InputFrame>,
        scene: Scene,
        frame: Option<&DecodedFrame>,
        now: Instant,
    ) {
        if matches!(self.phase, Phase::Drive)
            && !matches!(self.gesture, Some(Gesture::Pin { .. }))
        {
            if let Some(frame) = frame {
                if looks_like_ios_passcode(frame) {
                    if let Some(pad) = find_keypad(frame) {
                        self.pad = pad;
                    }
                }
            }
        }
        if scene != Scene::NoFrame {
            self.last_scene = scene;
        }
        let scene = if scene == Scene::NoFrame {
            self.last_scene
        } else {
            scene
        };

        if self.pump_gesture(tx, now) {
            return;
        }
        if matches!(self.phase, Phase::Done | Phase::Failed) {
            return;
        }
        if now.duration_since(self.started) > Duration::from_secs(90) {
            self.fail("не уложился в 90 с");
            return;
        }

        // Real Alfa error dialog is a compact card. Blue names in the
        // container list must not be treated as «ОК».
        if matches!(
            self.phase,
            Phase::PickPerson | Phase::TypeAppPin | Phase::WaitResult
        ) {
            if let Some(frame) = frame {
                if looks_like_alert(frame) {
                    self.dismiss_alert(frame, now);
                    return;
                }
            }
        }

        if now < self.until {
            return;
        }

        match self.phase {
            Phase::Drive => self.drive(tx, scene, now),
            Phase::TapApp => {
                self.status = "открываю Деньги".into();
                self.start_tap(DOCK_APP.0, DOCK_APP.1, now);
                self.phase = Phase::PickPerson;
                self.until = now + Duration::from_millis(1200);
            }
            Phase::PickPerson => self.pick_person(frame, now),
            Phase::TypeAppPin => self.type_app_pin(frame, now),
            Phase::WaitResult => self.wait_result(frame, now),
            Phase::Done | Phase::Failed => {}
        }
    }

    fn drive(&mut self, tx: &Sender<InputFrame>, scene: Scene, now: Instant) {
        self.cycles = self.cycles.saturating_add(1);
        if self.cycles > 18 {
            self.fail("не понял экран");
            return;
        }

        if self.allow_pin && scene != Scene::Home && scene != Scene::NoFrame {
            if scene == Scene::Asleep {
                self.status = "бужу".into();
                action(tx, SystemAction::Wake);
                self.until = now + Duration::from_millis(900);
                return;
            }
            self.status = "ввожу пароль".into();
            self.start_pin(now);
            self.allow_pin = false;
            let n = self.pin.chars().filter(|c| c.is_ascii_digit()).count() as u64;
            self.until = now + Duration::from_millis(220 * n + 1400);
            return;
        }

        match scene {
            Scene::NoFrame => {
                self.status = "жду кадр…".into();
                self.until = now + Duration::from_millis(250);
            }
            Scene::Asleep => {
                self.status = "бужу".into();
                action(tx, SystemAction::Wake);
                self.allow_pin = true;
                self.until = now + Duration::from_millis(1000);
            }
            Scene::Passcode => {
                if self.allow_pin {
                    self.status = "ввожу пароль".into();
                    self.start_pin(now);
                } else {
                    self.status = "сворачиваю приложение".into();
                    action(tx, SystemAction::Home);
                    self.until = now + Duration::from_millis(1000);
                }
            }
            Scene::Lock => {
                self.status = "смахиваю блокировку".into();
                self.allow_pin = true;
                self.start_swipe(0.50, 0.92, 0.50, 0.28, now, true);
                self.until = now + Duration::from_millis(1600);
            }
            Scene::Other => {
                self.status = "сворачиваю приложение".into();
                action(tx, SystemAction::Home);
                self.until = now + Duration::from_millis(1000);
            }
            Scene::Home => {
                self.allow_pin = false;
                self.phase = Phase::TapApp;
                self.until = now;
            }
        }
    }

    fn pick_person(&mut self, frame: Option<&DecodedFrame>, now: Instant) {
        let Some(frame) = frame else {
            self.status = "жду кадр…".into();
            self.until = now + Duration::from_millis(200);
            return;
        };
        if looks_like_home(frame) {
            self.phase = Phase::TapApp;
            self.until = now + Duration::from_millis(400);
            return;
        }
        let Some(hits) = self.ensure_ocr(frame, now) else {
            return;
        };
        let error = screen_has_error(hits);
        let ok = find_ok_hit(hits);
        let pin = screen_is_pin(hits);
        let yugov = find_yugov(hits);
        let container = screen_is_container(hits) || looks_like_container(frame);
        if error {
            if let Some((x, y)) = ok {
                self.status = "ошибка сервера — ОК".into();
                self.start_tap(x, y, now);
                self.invalidate_ocr();
                self.until = now + Duration::from_millis(900);
                return;
            }
        }
        if pin {
            self.stall = 0;
            self.invalidate_ocr();
            self.phase = Phase::TypeAppPin;
            self.until = now;
            return;
        }
        if let Some((x, y)) = yugov {
            self.stall = 0;
            self.status = "выбираю Югов Р.".into();
            self.start_tap(x, y, now);
            self.invalidate_ocr();
            self.phase = Phase::TypeAppPin;
            self.until = now + Duration::from_millis(1400);
            return;
        }
        if container {
            self.stall = self.stall.saturating_add(1);
            if self.stall > 6 {
                self.fail("не нашёл Югов Р. в списке");
                return;
            }
            self.status = "листаю список".into();
            self.start_swipe(0.50, 0.72, 0.50, 0.38, now, false);
            self.invalidate_ocr();
            self.until = now + Duration::from_millis(900);
            return;
        }
        self.stall = self.stall.saturating_add(1);
        if self.stall > 10 {
            self.fail("не нашёл список контейнеров");
            return;
        }
        self.status = "ищу Югов Р.…".into();
        self.until = now + Duration::from_millis(350);
    }

    fn type_app_pin(&mut self, frame: Option<&DecodedFrame>, now: Instant) {
        let Some(frame) = frame else {
            self.until = now + Duration::from_millis(200);
            return;
        };
        if looks_like_home(frame) {
            self.phase = Phase::TapApp;
            self.until = now + Duration::from_millis(400);
            return;
        }
        let Some(hits) = self.ensure_ocr(frame, now) else {
            return;
        };
        let error = screen_has_error(hits);
        let ok = find_ok_hit(hits);
        let container = screen_is_container(hits) || looks_like_container(frame);
        let pin = screen_is_pin(hits);
        let taps = pin_taps_from_ocr(hits, APP_PIN);
        if error {
            if let Some((x, y)) = ok {
                self.status = "ошибка сервера — ОК".into();
                self.start_tap(x, y, now);
                self.invalidate_ocr();
                self.until = now + Duration::from_millis(900);
                return;
            }
        }
        if container {
            self.phase = Phase::PickPerson;
            self.until = now;
            return;
        }
        if !pin {
            self.stall = self.stall.saturating_add(1);
            if self.stall > 12 {
                self.fail("нет экрана кода");
                return;
            }
            self.status = "жду экран кода…".into();
            self.until = now + Duration::from_millis(350);
            return;
        }
        if self.app_pin_tries >= 5 {
            self.fail("ошибка сервера повторяется");
            return;
        }
        let taps = taps.unwrap_or_else(|| digits_on_pad(&ALFA_PAD, APP_PIN));
        if taps.is_empty() {
            self.fail("не вижу цифры PIN");
            return;
        }
        self.app_pin_tries = self.app_pin_tries.saturating_add(1);
        self.stall = 0;
        self.status = "пароль 0805".into();
        self.gesture = Some(Gesture::Pin {
            taps,
            i: 0,
            release: false,
            next: now,
        });
        self.invalidate_ocr();
        self.phase = Phase::WaitResult;
        self.until = now + Duration::from_millis(2500);
    }

    fn wait_result(&mut self, frame: Option<&DecodedFrame>, now: Instant) {
        let Some(frame) = frame else {
            self.until = now + Duration::from_millis(200);
            return;
        };
        if looks_like_home(frame) {
            self.phase = Phase::TapApp;
            self.until = now + Duration::from_millis(400);
            return;
        }
        let Some(hits) = self.ensure_ocr(frame, now) else {
            return;
        };
        let error = screen_has_error(hits) || looks_like_alert(frame);
        let container = screen_is_container(hits) || looks_like_container(frame);
        let pin = screen_is_pin(hits);
        if error {
            self.dismiss_alert(frame, now);
            return;
        }
        if container {
            self.phase = Phase::PickPerson;
            self.until = now;
            return;
        }
        if pin {
            self.status = "жду ответ сервера".into();
            self.until = now + Duration::from_millis(700);
            return;
        }
        self.phase = Phase::Done;
        self.status = "готово".into();
    }

    fn dismiss_alert(&mut self, frame: &DecodedFrame, now: Instant) {
        self.status = "ошибка сервера — ОК".into();
        let (x, y) = find_ok_hit(&self.hits)
            .or_else(|| find_alert_ok(frame))
            .unwrap_or(ALERT_OK);
        self.start_tap(x, y, now);
        if matches!(self.phase, Phase::WaitResult) {
            self.phase = Phase::TypeAppPin;
        }
        self.invalidate_ocr();
        self.until = now + Duration::from_millis(900);
    }

    fn ensure_ocr(&mut self, frame: &DecodedFrame, now: Instant) -> Option<&[crate::ocr::Hit]> {
        if let Some(hits) = crate::ocr::take() {
            self.hits = hits;
            self.ocr_pending = false;
            self.last_ocr = now;
        }
        let fresh = !self.hits.is_empty()
            && now.duration_since(self.last_ocr) < Duration::from_millis(600);
        if !fresh && !self.ocr_pending {
            crate::ocr::submit(frame);
            self.ocr_pending = true;
        }
        if self.hits.is_empty() {
            self.status = "читаю экран…".into();
            self.until = now + Duration::from_millis(80);
            return None;
        }
        Some(&self.hits)
    }

    fn invalidate_ocr(&mut self) {
        self.hits.clear();
        self.ocr_pending = false;
        self.last_ocr = Instant::now() - Duration::from_secs(30);
    }

    fn fail(&mut self, why: &str) {
        self.phase = Phase::Failed;
        self.gesture = None;
        self.status = format!("авто: {why}");
    }

    fn start_swipe(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        now: Instant,
        then_pin: bool,
    ) {
        let n = 14usize;
        let pts = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
            })
            .collect();
        self.gesture = Some(Gesture::Swipe {
            pts,
            i: 0,
            next: now,
            then_pin,
        });
    }

    fn start_tap(&mut self, x: f32, y: f32, now: Instant) {
        self.gesture = Some(Gesture::Tap {
            x,
            y,
            release: false,
            next: now,
        });
    }

    fn start_pin(&mut self, now: Instant) {
        if self.pin_sent {
            return;
        }
        self.pin_sent = true;
        self.start_digits(&self.pin.clone(), now);
    }

    fn start_digits(&mut self, digits: &str, now: Instant) {
        let taps = digits
            .chars()
            .filter_map(|c| c.to_digit(10).map(|d| self.pad[d as usize]))
            .collect::<Vec<_>>();
        if taps.is_empty() {
            return;
        }
        self.gesture = Some(Gesture::Pin {
            taps,
            i: 0,
            release: false,
            next: now,
        });
    }

    fn pump_gesture(&mut self, tx: &Sender<InputFrame>, now: Instant) -> bool {
        let mut start_pin = false;
        let mut clear = false;
        let mut busy = false;

        match self.gesture.as_mut() {
            None => return false,
            Some(Gesture::Swipe {
                pts,
                i,
                next,
                then_pin,
            }) => {
                if now < *next {
                    return true;
                }
                if *i >= pts.len() {
                    start_pin = *then_pin;
                    clear = true;
                } else {
                    let (x, y) = pts[*i];
                    let first = *i == 0;
                    let last = *i + 1 == pts.len();
                    *i += 1;
                    *next = now + Duration::from_millis(20);
                    if first {
                        touch(tx, TouchPhase::Down, x, y);
                    } else if last {
                        touch(tx, TouchPhase::Move, x, y);
                        touch(tx, TouchPhase::Up, x, y);
                    } else {
                        touch(tx, TouchPhase::Move, x, y);
                    }
                    busy = true;
                }
            }
            Some(Gesture::Tap {
                x,
                y,
                release,
                next,
            }) => {
                if now < *next {
                    return true;
                }
                if !*release {
                    touch(tx, TouchPhase::Down, *x, *y);
                    *release = true;
                    *next = now + Duration::from_millis(60);
                    busy = true;
                } else {
                    touch(tx, TouchPhase::Up, *x, *y);
                    clear = true;
                }
            }
            Some(Gesture::Pin {
                taps,
                i,
                release,
                next,
            }) => {
                if now < *next {
                    return true;
                }
                if *i >= taps.len() {
                    self.allow_pin = false;
                    clear = true;
                } else {
                    let (x, y) = taps[*i];
                    if !*release {
                        touch(tx, TouchPhase::Down, x, y);
                        *release = true;
                        *next = now + Duration::from_millis(90);
                    } else {
                        touch(tx, TouchPhase::Up, x, y);
                        *release = false;
                        *i += 1;
                        *next = now + Duration::from_millis(90);
                    }
                    busy = true;
                }
            }
        }

        if clear {
            self.gesture = None;
        }
        if start_pin {
            self.allow_pin = true;
            self.status = "жду клавиатуру PIN".into();
            self.until = now + Duration::from_millis(850);
            return false;
        }
        busy && !clear
    }
}

pub fn classify(frame: Option<&DecodedFrame>) -> Scene {
    let Some(frame) = frame else {
        return Scene::NoFrame;
    };
    if frame.width < 8 || frame.height < 8 || frame.buf.len() < frame.width * frame.height {
        return Scene::NoFrame;
    }
    if frame.width > frame.height {
        return Scene::Other;
    }

    let mean = sample_mean(frame, 0.08, 0.08, 0.92, 0.92, 8, 14);
    if mean < 10.0 {
        return Scene::Asleep;
    }
    if looks_like_home(frame) {
        return Scene::Home;
    }
    if looks_like_ios_passcode(frame) {
        return Scene::Passcode;
    }
    if looks_like_lock(frame) {
        return Scene::Lock;
    }
    Scene::Other
}

fn looks_like_ios_passcode(frame: &DecodedFrame) -> bool {
    if looks_like_home(frame) || looks_like_container(frame) || looks_like_alert(frame) {
        return false;
    }
    // Bank apps usually put a logo / coloured header at the top.
    if region_chroma(frame, 0.12, 0.05, 0.88, 0.16, 4, 3) > 38.0 {
        return false;
    }
    find_keypad(frame).is_some()
}

fn looks_like_lock(frame: &DecodedFrame) -> bool {
    if looks_like_container(frame) || looks_like_alert(frame) {
        return false;
    }
    region_chroma(frame, 0.16, 0.18, 0.84, 0.72, 4, 6) > 22.0
}

fn looks_like_home(frame: &DecodedFrame) -> bool {
    let xs = [0.20, 0.36, 0.50, 0.64, 0.80];
    let mut colorful = 0u8;
    for x in xs {
        let p = sample(frame, x, 0.925);
        if chroma(p) > 28.0 && luma(p) > 35.0 {
            colorful += 1;
        }
    }
    colorful >= 3
}

fn looks_like_container(frame: &DecodedFrame) -> bool {
    if looks_like_home(frame) {
        return false;
    }
    let header = sample_mean(frame, 0.18, 0.10, 0.82, 0.20, 4, 2);
    let mid = sample_mean(frame, 0.18, 0.38, 0.82, 0.55, 4, 3);
    let list_chroma = region_chroma(frame, 0.16, 0.22, 0.84, 0.72, 3, 8);
    (header - mid).abs() < 40.0 && list_chroma > 10.0
}

fn looks_like_alert(frame: &DecodedFrame) -> bool {
    if looks_like_home(frame) || looks_like_container(frame) {
        return false;
    }
    compact_alert_card(frame)
}

fn compact_alert_card(frame: &DecodedFrame) -> bool {
    let card = sample_mean(frame, 0.22, 0.42, 0.78, 0.57, 5, 4);
    let left = sample_mean(frame, 0.00, 0.42, 0.10, 0.57, 2, 3);
    let right = sample_mean(frame, 0.90, 0.42, 1.00, 0.57, 2, 3);
    let side = (left + right) * 0.5;
    let above = sample_mean(frame, 0.22, 0.16, 0.78, 0.32, 4, 2);
    let below = sample_mean(frame, 0.22, 0.64, 0.78, 0.80, 4, 3);
    card > side + 12.0 && card > above + 12.0 && card > below + 8.0
}

fn find_alert_ok(frame: &DecodedFrame) -> Option<(f32, f32)> {
    if !compact_alert_card(frame) {
        return None;
    }
    let mut sx = 0.0_f32;
    let mut sy = 0.0_f32;
    let mut n = 0.0_f32;
    let mut y = 0.48_f32;
    while y <= 0.60 {
        let mut x = 0.38_f32;
        while x <= 0.62 {
            let p = sample(frame, x, y);
            let r = ((p >> 16) & 0xff) as f32;
            let g = ((p >> 8) & 0xff) as f32;
            let b = (p & 0xff) as f32;
            if b > 80.0 && b > r + 18.0 && b > g + 8.0 {
                sx += x;
                sy += y;
                n += 1.0;
            }
            x += 0.012;
        }
        y += 0.006;
    }
    if n >= 4.0 {
        Some((sx / n, sy / n))
    } else {
        None
    }
}

fn find_yugov(hits: &[crate::ocr::Hit]) -> Option<(f32, f32)> {
    hits.iter()
        .find(|h| {
            let t = crate::ocr::fold(&h.text);
            t.contains("югов")
                || t.contains("yugov")
                || t.contains("iogov")
                || t.contains("юговр")
        })
        .map(|h| (h.cx, h.cy))
}

fn find_ok_hit(hits: &[crate::ocr::Hit]) -> Option<(f32, f32)> {
    hits.iter()
        .find(|h| {
            let t = crate::ocr::fold(&h.text);
            (t == "ок" || t == "ok" || (t.ends_with("ок") && t.len() <= 4))
                && h.cy > 0.40
                && h.cy < 0.65
                && h.cx > 0.30
                && h.cx < 0.70
        })
        .map(|h| (h.cx, h.cy))
}

fn screen_is_container(hits: &[crate::ocr::Hit]) -> bool {
    let t = crate::ocr::joined(hits);
    t.contains("container")
        || t.contains("отменить")
        || t.contains("поумолчанию")
        || (t.contains("деньги") && t.contains("select"))
}

fn screen_is_pin(hits: &[crate::ocr::Hit]) -> bool {
    let t = crate::ocr::joined(hits);
    t.contains("введитекод")
        || t.contains("забыликод")
        || (t.contains("введите") && t.contains("код"))
}

fn screen_has_error(hits: &[crate::ocr::Hit]) -> bool {
    let t = crate::ocr::joined(hits);
    t.contains("ошибка") || (t.contains("соединен") && t.contains("сервер"))
}

fn pin_taps_from_ocr(hits: &[crate::ocr::Hit], digits: &str) -> Option<Vec<(f32, f32)>> {
    let mut pos = [None; 10];
    for h in hits {
        let t = h.text.trim();
        if t.len() != 1 || h.cy < 0.42 {
            continue;
        }
        if let Some(d) = t.chars().next().and_then(|c| c.to_digit(10)) {
            let slot = &mut pos[d as usize];
            if slot.map(|(_, y)| h.cy > y).unwrap_or(true) {
                *slot = Some((h.cx, h.cy));
            }
        }
    }
    let mut taps = Vec::new();
    for c in digits.chars() {
        let d = c.to_digit(10)? as usize;
        taps.push(pos[d]?);
    }
    Some(taps)
}

fn digits_on_pad(pad: &[(f32, f32); 10], digits: &str) -> Vec<(f32, f32)> {
    digits
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| pad[d as usize]))
        .collect()
}

fn find_keypad(frame: &DecodedFrame) -> Option<[(f32, f32); 10]> {
    let mut ys = Vec::new();
    let mut y = 0.36_f32;
    let mut best_y = 0.0;
    let mut best_s = 0.0_f32;
    let mut last_kept = -1.0_f32;
    while y <= 0.86 {
        let s = button_score(frame, 0.50, y);
        if s > best_s {
            best_s = s;
            best_y = y;
        }
        if s < best_s * 0.72 && best_s > 7.0 && best_y - last_kept > 0.06 {
            ys.push(best_y);
            last_kept = best_y;
            best_s = 0.0;
        }
        y += 0.008;
    }
    if best_s > 7.0 && best_y - last_kept > 0.06 {
        ys.push(best_y);
    }
    if ys.len() < 4 {
        return None;
    }
    let rows = &ys[ys.len() - 4..];
    let row0 = rows[0];
    let mut xs = [0.25_f32, 0.50, 0.75];
    let mut col_scores = [0.0_f32; 3];
    let mut x = 0.14_f32;
    while x <= 0.86 {
        let s = button_score(frame, x, row0);
        let slot = if x < 0.38 {
            0
        } else if x < 0.62 {
            1
        } else {
            2
        };
        if s > col_scores[slot] {
            col_scores[slot] = s;
            xs[slot] = x;
        }
        x += 0.01;
    }
    if col_scores.iter().any(|s| *s < 6.0) {
        return None;
    }
    Some([
        (xs[1], rows[3]),
        (xs[0], rows[0]),
        (xs[1], rows[0]),
        (xs[2], rows[0]),
        (xs[0], rows[1]),
        (xs[1], rows[1]),
        (xs[2], rows[1]),
        (xs[0], rows[2]),
        (xs[1], rows[2]),
        (xs[2], rows[2]),
    ])
}

fn button_score(frame: &DecodedFrame, x: f32, y: f32) -> f32 {
    let c = patch_luma(frame, x, y);
    let ring = (patch_luma(frame, x + 0.10, y)
        + patch_luma(frame, x - 0.10, y)
        + patch_luma(frame, x, y + 0.055)
        + patch_luma(frame, x, y - 0.055))
        / 4.0;
    (c - ring).max(0.0)
}

fn sample(frame: &DecodedFrame, nx: f32, ny: f32) -> u32 {
    let w = frame.width;
    let h = frame.height;
    let x = ((nx.clamp(0.0, 0.999) * w as f32) as usize).min(w - 1);
    let y = ((ny.clamp(0.0, 0.999) * h as f32) as usize).min(h - 1);
    frame.buf[y * w + x]
}

fn luma(p: u32) -> f32 {
    let r = ((p >> 16) & 0xff) as f32;
    let g = ((p >> 8) & 0xff) as f32;
    let b = (p & 0xff) as f32;
    0.299 * r + 0.587 * g + 0.114 * b
}

fn chroma(p: u32) -> f32 {
    let r = ((p >> 16) & 0xff) as f32;
    let g = ((p >> 8) & 0xff) as f32;
    let b = (p & 0xff) as f32;
    r.max(g).max(b) - r.min(g).min(b)
}

fn patch_luma(frame: &DecodedFrame, nx: f32, ny: f32) -> f32 {
    let mut s = 0.0_f32;
    let mut n = 0.0_f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            s += luma(sample(
                frame,
                nx + dx as f32 * 0.012,
                ny + dy as f32 * 0.010,
            ));
            n += 1.0;
        }
    }
    s / n
}

fn region_chroma(
    frame: &DecodedFrame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    nx: i32,
    ny: i32,
) -> f32 {
    let mut s = 0.0_f32;
    let mut n = 0.0_f32;
    for iy in 0..ny {
        let y = y0 + (y1 - y0) * (iy as f32 + 0.5) / ny as f32;
        for ix in 0..nx {
            let x = x0 + (x1 - x0) * (ix as f32 + 0.5) / nx as f32;
            s += chroma(sample(frame, x, y));
            n += 1.0;
        }
    }
    s / n.max(1.0)
}

fn sample_mean(
    frame: &DecodedFrame,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    nx: i32,
    ny: i32,
) -> f32 {
    let mut s = 0.0_f32;
    let mut n = 0.0_f32;
    for iy in 0..ny {
        let y = y0 + (y1 - y0) * (iy as f32 + 0.5) / ny as f32;
        for ix in 0..nx {
            let x = x0 + (x1 - x0) * (ix as f32 + 0.5) / nx as f32;
            s += luma(sample(frame, x, y));
            n += 1.0;
        }
    }
    s / n.max(1.0)
}

fn touch(tx: &Sender<InputFrame>, phase: TouchPhase, x: f32, y: f32) {
    let _ = tx.send(InputFrame::new(
        MessageType::InputTouch,
        protocol::encode_touch(phase, 0, x, y),
    ));
}

fn action(tx: &Sender<InputFrame>, action: SystemAction) {
    let _ = tx.send(InputFrame::new(
        MessageType::SystemAction,
        protocol::encode_system_action(action),
    ));
}

