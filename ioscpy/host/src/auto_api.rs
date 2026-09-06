//! UI-tree automation: dump labels from the tweak, tap by text.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::input::InputFrame;
use crate::protocol::{self, MessageType, SystemAction, TouchPhase};

pub const LOCK_PIN_DEFAULT: &str = "956123";
pub const APP_PIN_DEFAULT: &str = "0805";
pub const PROFILE_DEFAULT: &str = "Югов";
pub const APP_DEFAULT: &str = "Деньги";

#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub class: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub bundle: String,
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    #[allow(dead_code)]
    pub w: f32,
    #[serde(default)]
    #[allow(dead_code)]
    pub h: f32,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct Dump {
    #[serde(default)]
    pub bundle: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
}

impl Dump {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    pub fn joined(&self) -> String {
        fold(
            &self
                .nodes
                .iter()
                .map(|n| n.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn find(&self, needle: &str) -> Option<&Node> {
        let n = fold(needle);
        if n.is_empty() {
            return None;
        }
        self.nodes
            .iter()
            .filter(|node| fold(&node.text).contains(&n) || fold(&node.id).contains(&n))
            .min_by_key(|node| fold(&node.text).len())
    }

    pub fn find_digit(&self, d: char) -> Option<&Node> {
        let want = d.to_string();
        self.nodes
            .iter()
            .filter(|n| n.text.trim() == want && n.y > 0.38)
            .min_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn profiles(&self) -> Vec<String> {
        let mut out = Vec::new();
        for n in &self.nodes {
            let t = n.text.trim();
            if t.chars().count() < 4 || n.y < 0.16 || n.y > 0.86 {
                continue;
            }
            let f = fold(t);
            if f.contains("отменить")
                || f.contains("container")
                || f.contains("select")
                || f == "деньги"
                || f.contains("введите")
                || f.contains("забыли")
            {
                continue;
            }
            if !t.chars().any(|c| c.is_alphabetic()) {
                continue;
            }
            if !out.iter().any(|s: &String| s == t) {
                out.push(t.to_string());
            }
        }
        out
    }

    pub fn is_pin(&self) -> bool {
        let t = self.joined();
        t.contains("введитекод")
            || t.contains("забыликод")
            || (t.contains("введите") && t.contains("код"))
    }

    pub fn is_container(&self) -> bool {
        let t = self.joined();
        t.contains("container") || t.contains("отменить") || t.contains("поумолчанию")
    }

    pub fn is_error(&self) -> bool {
        let t = self.joined();
        t.contains("ошибка") || (t.contains("соединен") && t.contains("сервер"))
    }

    pub fn find_ok(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| {
            let t = fold(&n.text);
            (t == "ок" || t == "ok") && n.y > 0.35 && n.y < 0.70
        })
    }
}

pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c == 'ё' { 'е' } else { c })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Drive,
    Work,
    Done,
    Failed,
}

pub struct Job {
    lock_pin: String,
    app_pin: String,
    profile: String,
    app_name: String,
    phase: Phase,
    status: String,
    started: Instant,
    until: Instant,
    dump_at: Instant,
    waiting_dump: bool,
    lock_sent: bool,
    app_tries: u8,
    stall: u8,
    taps: Vec<(f32, f32)>,
    tap_i: usize,
    tap_down: bool,
    tap_at: Instant,
}

impl Job {
    pub fn idle() -> Self {
        Self {
            lock_pin: LOCK_PIN_DEFAULT.into(),
            app_pin: APP_PIN_DEFAULT.into(),
            profile: PROFILE_DEFAULT.into(),
            app_name: APP_DEFAULT.into(),
            phase: Phase::Done,
            status: String::new(),
            started: Instant::now(),
            until: Instant::now(),
            dump_at: Instant::now() - Duration::from_secs(30),
            waiting_dump: false,
            lock_sent: false,
            app_tries: 0,
            stall: 0,
            taps: Vec::new(),
            tap_i: 0,
            tap_down: false,
            tap_at: Instant::now(),
        }
    }

    pub fn start(lock_pin: &str, app_pin: &str, profile: &str, app_name: &str) -> Self {
        let mut j = Self::idle();
        j.lock_pin = digits(lock_pin, LOCK_PIN_DEFAULT);
        j.app_pin = digits(app_pin, APP_PIN_DEFAULT);
        j.profile = profile.trim().to_string();
        if j.profile.is_empty() {
            j.profile = PROFILE_DEFAULT.into();
        }
        j.app_name = app_name.trim().to_string();
        if j.app_name.is_empty() {
            j.app_name = APP_DEFAULT.into();
        }
        j.phase = Phase::Drive;
        j.started = Instant::now();
        j.until = Instant::now();
        j.status = "смотрю UI…".into();
        j
    }

    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Drive | Phase::Work) || self.tap_i < self.taps.len()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn tick(&mut self, tx: &Sender<InputFrame>, dump: Option<&Dump>, now: Instant) {
        if matches!(self.phase, Phase::Done | Phase::Failed) {
            return;
        }
        if now.duration_since(self.started) > Duration::from_secs(90) {
            self.fail("не уложился в 90 с");
            return;
        }
        if self.pump_taps(tx, now) {
            return;
        }
        if now < self.until {
            return;
        }
        let Some(dump) = dump else {
            self.need_dump(tx, now);
            return;
        };

        if dump.is_error() {
            if let Some(n) = dump.find_ok() {
                self.status = "ошибка сервера — ОК".into();
                self.queue_tap(n.x, n.y, now);
                self.until = now + Duration::from_millis(900);
                self.need_dump(tx, now);
                return;
            }
        }

        let has_pad = dump.find_digit('1').is_some() && dump.find_digit('0').is_some();
        if has_pad && dump.is_pin() {
            if self.app_tries >= 5 {
                self.fail("ошибка сервера повторяется");
                return;
            }
            self.app_tries = self.app_tries.saturating_add(1);
            let pin = self.app_pin.clone();
            self.status = format!("пароль {pin}");
            if self.queue_digits(dump, &pin, now) {
                self.until = now + Duration::from_millis(2200);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
            } else {
                self.fail("не вижу цифры PIN");
            }
            return;
        }
        if has_pad && !self.lock_sent && !dump.is_pin() {
            let pin = self.lock_pin.clone();
            self.status = "PIN блокировки".into();
            if self.queue_digits(dump, &pin, now) {
                self.lock_sent = true;
                self.until = now + Duration::from_millis(1600);
                self.need_dump(tx, now);
            } else {
                self.status = "смахиваю блокировку".into();
                swipe(tx, 0.50, 0.90, 0.50, 0.28);
                self.until = now + Duration::from_millis(1200);
                self.need_dump(tx, now);
            }
            return;
        }

        if dump.is_container() {
            if let Some(n) = dump.find(&self.profile) {
                self.status = format!("выбираю {}", self.profile);
                self.queue_tap(n.x, n.y, now);
                self.until = now + Duration::from_millis(1200);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
                self.stall = 0;
                return;
            }
            self.stall = self.stall.saturating_add(1);
            if self.stall > 6 {
                self.fail(&format!("нет профиля «{}»", self.profile));
                return;
            }
            self.status = "листаю список".into();
            swipe(tx, 0.50, 0.70, 0.50, 0.38);
            self.until = now + Duration::from_millis(800);
            self.need_dump(tx, now);
            return;
        }

        if dump.bundle == "com.apple.springboard" {
            if let Some(n) = dump.find(&self.app_name) {
                self.status = format!("открываю {}", self.app_name);
                self.queue_tap(n.x, n.y, now);
                self.until = now + Duration::from_millis(1400);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
                self.stall = 0;
                return;
            }
            self.stall = self.stall.saturating_add(1);
            if self.stall > 8 {
                self.fail(&format!("нет иконки «{}»", self.app_name));
                return;
            }
            self.status = "ищу приложение…".into();
            self.need_dump(tx, now);
            self.until = now + Duration::from_millis(400);
            return;
        }

        if dump.is_pin() {
            self.need_dump(tx, now);
            self.until = now + Duration::from_millis(500);
            return;
        }

        if self.phase == Phase::Drive {
            self.status = "бужу / домой".into();
            action(tx, SystemAction::Wake);
            action(tx, SystemAction::Home);
            self.until = now + Duration::from_millis(900);
            self.need_dump(tx, now);
            self.stall = self.stall.saturating_add(1);
            if self.stall > 10 {
                self.fail("не понял экран");
            }
            return;
        }

        self.phase = Phase::Done;
        self.status = "готово".into();
    }

    fn need_dump(&mut self, tx: &Sender<InputFrame>, now: Instant) {
        if self.waiting_dump && now.duration_since(self.dump_at) < Duration::from_millis(700) {
            return;
        }
        self.waiting_dump = true;
        self.dump_at = now;
        let _ = tx.send(InputFrame::new(MessageType::UiDump, Vec::new()));
    }

    pub fn dump_arrived(&mut self) {
        self.waiting_dump = false;
    }

    fn queue_tap(&mut self, x: f32, y: f32, now: Instant) {
        if self.taps.is_empty() {
            self.tap_i = 0;
            self.tap_down = false;
            self.tap_at = now;
        }
        self.taps.push((x, y));
    }

    fn queue_digits(&mut self, dump: &Dump, pin: &str, now: Instant) -> bool {
        let mut pts = Vec::new();
        for c in pin.chars() {
            if let Some(n) = dump.find_digit(c) {
                pts.push((n.x, n.y));
            } else {
                return false;
            }
        }
        for (x, y) in pts {
            self.queue_tap(x, y, now);
        }
        true
    }

    fn pump_taps(&mut self, tx: &Sender<InputFrame>, now: Instant) -> bool {
        if self.tap_i >= self.taps.len() {
            if !self.taps.is_empty() {
                self.taps.clear();
                self.tap_i = 0;
                self.tap_down = false;
            }
            return false;
        }
        if now < self.tap_at {
            return true;
        }
        let (x, y) = self.taps[self.tap_i];
        if !self.tap_down {
            let _ = tx.send(InputFrame::new(
                MessageType::InputTouch,
                protocol::encode_touch(TouchPhase::Down, 0, x, y),
            ));
            self.tap_down = true;
            self.tap_at = now + Duration::from_millis(70);
        } else {
            let _ = tx.send(InputFrame::new(
                MessageType::InputTouch,
                protocol::encode_touch(TouchPhase::Up, 0, x, y),
            ));
            self.tap_down = false;
            self.tap_i += 1;
            self.tap_at = now + Duration::from_millis(90);
        }
        true
    }

    fn fail(&mut self, why: &str) {
        self.phase = Phase::Failed;
        self.status = format!("авто: {why}");
    }
}

fn digits(s: &str, fallback: &str) -> String {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.is_empty() {
        fallback.into()
    } else {
        d
    }
}

fn swipe(tx: &Sender<InputFrame>, x0: f32, y0: f32, x1: f32, y1: f32) {
    let n = 10u32;
    let _ = tx.send(InputFrame::new(
        MessageType::InputTouch,
        protocol::encode_touch(TouchPhase::Down, 0, x0, y0),
    ));
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let x = x0 + (x1 - x0) * t;
        let y = y0 + (y1 - y0) * t;
        let _ = tx.send(InputFrame::new(
            MessageType::InputTouch,
            protocol::encode_touch(TouchPhase::Move, 0, x, y),
        ));
    }
    let _ = tx.send(InputFrame::new(
        MessageType::InputTouch,
        protocol::encode_touch(TouchPhase::Up, 0, x1, y1),
    ));
}

fn action(tx: &Sender<InputFrame>, action: SystemAction) {
    let _ = tx.send(InputFrame::new(
        MessageType::SystemAction,
        protocol::encode_system_action(action),
    ));
}
