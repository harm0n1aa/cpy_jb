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

/// Fallback iOS lock passcode pad (notched iPhone). Index = digit 0..=9.
const LOCK_PAD: [(f32, f32); 10] = [
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

const LOG_CAP: usize = 120;

#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
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

    /// Compact one-line screen snapshot for the action log.
    pub fn brief(&self) -> String {
        let has_pad = self.has_digit_pad();
        let mut flags = Vec::new();
        if self.is_error() {
            flags.push("error");
        }
        if self.is_pin() {
            flags.push("pin");
        }
        if self.is_container() {
            flags.push("container");
        }
        if has_pad {
            flags.push("pad");
        }
        if self.is_springboard() {
            flags.push("springboard");
        }
        let labels: Vec<String> = self
            .nodes
            .iter()
            .filter(|n| !n.text.trim().is_empty())
            .take(14)
            .map(|n| {
                format!(
                    "{}@{:.2},{:.2}",
                    trunc(n.text.trim(), 18),
                    n.x,
                    n.y
                )
            })
            .collect();
        let b = if self.bundle.is_empty() {
            "?"
        } else {
            self.bundle.rsplit('.').next().unwrap_or(self.bundle.as_str())
        };
        format!(
            "UI {} [{}] n={} {}",
            b,
            flags.join("|"),
            self.nodes.len(),
            labels.join(" · ")
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
        let want = d;
        self.nodes
            .iter()
            .filter(|n| {
                if n.y <= 0.35 {
                    return false;
                }
                first_digit(&n.text) == Some(want)
            })
            .min_by(|a, b| {
                // Prefer compact keys near the classic pad band.
                let da = (a.y - 0.60).abs();
                let db = (b.y - 0.60).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    pub fn has_digit_pad(&self) -> bool {
        self.find_digit('1').is_some() && self.find_digit('0').is_some()
    }

    pub fn is_springboard(&self) -> bool {
        self.bundle == "com.apple.springboard"
    }

    /// Icon / label for an app on the home screen (ignores bare bundle-id noise).
    pub fn find_app_icon(&self, name: &str) -> Option<&Node> {
        let n = fold(name);
        if n.is_empty() {
            return None;
        }
        self.nodes
            .iter()
            .filter(|node| {
                let f = fold(&node.text);
                if f.is_empty() {
                    return false;
                }
                // Prefer display name; allow "Деньги com.foo" style labels.
                f == n
                    || f.starts_with(&n)
                    || (f.contains(&n) && !n.contains("comapple"))
            })
            .min_by_key(|node| {
                let f = fold(&node.text);
                // Prefer shorter / exact display names over "Name + bundle".
                (f.len() as i32, (f != n) as i32)
            })
    }

    /// Only when the app shows the container picker — never SpringBoard icons.
    pub fn profiles(&self) -> Vec<String> {
        if !self.is_container() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for n in &self.nodes {
            let t = n.text.trim();
            if t.chars().count() < 3 || n.y < 0.12 || n.y > 0.90 {
                continue;
            }
            if looks_like_system_chrome(t) {
                continue;
            }
            let f = fold(t);
            if f.contains("отменить")
                || f.contains("container")
                || f.contains("select")
                || f.contains("поумолчанию")
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
            || t.contains("кодпароль")
            || (t.contains("введите") && t.contains("код"))
            || t.contains("enterpasscode")
            || t.contains("passcode")
    }

    /// Lock-screen passcode UI (even when digit nodes are missing from the tree).
    pub fn is_lock_passcode(&self) -> bool {
        let t = self.joined();
        t.contains("кодпароль")
            || (t.contains("отменить") && (t.contains("sos") || t.contains("экстренн")))
            || (self.is_springboard() && self.is_pin())
            || (self.is_pin()
                && self.nodes.iter().any(|n| {
                    let c = n.class.to_lowercase();
                    c.contains("passcode") || c.contains("numberpad")
                }))
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

fn looks_like_system_chrome(t: &str) -> bool {
    let f = fold(t);
    if f.contains("comapple") || t.contains("com.") {
        return true;
    }
    matches!(
        f.as_str(),
        "поиск"
            | "spotlightpill"
            | "pagecontrol"
            | "homescreenicons"
            | "домашнийэкран"
            | "погода"
            | "календарь"
            | "отменить"
            | "sos"
    ) || f.starts_with("homescreen")
        || f.contains("spotlight")
}

fn first_digit(s: &str) -> Option<char> {
    s.chars().find(|c| c.is_ascii_digit())
}

fn trunc(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Drive,
    Work,
    Done,
    Failed,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Drive => "drive",
            Phase::Work => "work",
            Phase::Done => "done",
            Phase::Failed => "fail",
        }
    }
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
    unlock_tries: u8,
    page_tries: u8,
    stall: u8,
    /// After wake/swipe, ignore dump until a fresh UiDumpResult arrives.
    stale_dump: bool,
    taps: Vec<(f32, f32, String)>,
    tap_i: usize,
    tap_down: bool,
    tap_at: Instant,
    log: Vec<String>,
    last_brief: String,
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
            unlock_tries: 0,
            page_tries: 0,
            stall: 0,
            stale_dump: false,
            taps: Vec::new(),
            tap_i: 0,
            tap_down: false,
            tap_at: Instant::now(),
            log: Vec::new(),
            last_brief: String::new(),
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
        j.note(&format!(
            "START app={} profile={} lock_pin={} app_pin={}",
            j.app_name, j.profile, j.lock_pin, j.app_pin
        ));
        j
    }

    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Drive | Phase::Work) || self.tap_i < self.taps.len()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn log_lines(&self) -> &[String] {
        &self.log
    }

    /// Record a UI dump even when auto is idle (manual «считать UI»).
    pub fn note_dump(&mut self, dump: &Dump) {
        let brief = dump.brief();
        if brief == self.last_brief {
            return;
        }
        self.last_brief = brief.clone();
        self.note(&brief);
        let profiles = dump.profiles();
        if !profiles.is_empty() {
            self.note(&format!("profiles: {}", profiles.join(", ")));
        }
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
        if self.stale_dump {
            self.need_dump(tx, now);
            return;
        }
        let Some(dump) = dump else {
            self.need_dump(tx, now);
            return;
        };

        let brief = dump.brief();
        if brief != self.last_brief {
            self.last_brief = brief.clone();
            self.note(&brief);
        }

        if dump.is_error() {
            if let Some(n) = dump.find_ok() {
                self.status = "ошибка сервера — ОК".into();
                self.decide(
                    "error-dialog",
                    &format!("вижу ошибку → tap ОК «{}»", n.text.trim()),
                    n,
                    now,
                );
                self.until = now + Duration::from_millis(900);
                self.need_dump(tx, now);
                return;
            }
            self.note("SEE error but no OK button in mid-screen");
        }

        let has_pad = dump.has_digit_pad();
        let on_sb = dump.is_springboard();
        let lock_ui = dump.is_lock_passcode() || (dump.is_pin() && !self.lock_sent);

        // Until lock_sent: digit pad OR lock-passcode title → enter lock PIN
        // (geometric fallback if accessibility tree has no digit nodes).
        if !self.lock_sent && (has_pad || lock_ui) {
            let pin = self.lock_pin.clone();
            self.status = "PIN блокировки".into();
            self.note(&format!(
                "DECIDE lock-pin pad={} lock_ui={} pin-text={} pin={}",
                has_pad,
                lock_ui,
                dump.is_pin(),
                pin
            ));
            if self.queue_digits(dump, &pin, "lock-pin", now) {
                self.lock_sent = true;
                self.unlock_tries = 0;
                self.mark_stale(now, 1800);
                self.need_dump(tx, now);
            } else {
                self.fail("не собрал PIN блокировки");
            }
            return;
        }

        // App PIN only after unlock.
        if has_pad && self.lock_sent && (dump.is_pin() || !on_sb) {
            if self.app_tries >= 5 {
                self.fail("ошибка сервера повторяется");
                return;
            }
            self.app_tries = self.app_tries.saturating_add(1);
            let pin = self.app_pin.clone();
            self.status = format!("пароль {pin}");
            self.note(&format!(
                "DECIDE app-pin try={} reason=pad after unlock pin={}",
                self.app_tries, pin
            ));
            if self.queue_digits(dump, &pin, "app-pin", now) {
                self.mark_stale(now, 2200);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
            } else {
                self.fail("не вижу цифры PIN приложения");
            }
            return;
        }

        // Locked / asleep: wake hard, then swipe up for passcode.
        if !self.lock_sent {
            let home_unlocked = on_sb && !has_pad && !dump.is_pin() && dump.nodes.len() >= 12;
            if home_unlocked {
                self.lock_sent = true;
                self.note("DECIDE already-unlocked: SpringBoard home without pad");
            } else {
                self.unlock_tries = self.unlock_tries.saturating_add(1);
                if self.unlock_tries > 12 {
                    self.fail("не разблокировал экран (нет PIN-пада)");
                    return;
                }
                self.status = "разблокировка…".into();
                if self.unlock_tries <= 2 {
                    self.note(&format!(
                        "DECIDE wake try={} — full backlight wake",
                        self.unlock_tries
                    ));
                    action(tx, SystemAction::Wake);
                    self.note("ACT Wake");
                    // Small tap so CoverSheet becomes interactive after dim wake.
                    let _ = tx.send(InputFrame::new(
                        MessageType::InputTouch,
                        protocol::encode_touch(TouchPhase::Down, 0, 0.50, 0.55),
                    ));
                    let _ = tx.send(InputFrame::new(
                        MessageType::InputTouch,
                        protocol::encode_touch(TouchPhase::Up, 0, 0.50, 0.55),
                    ));
                    self.note("ACT tap 0.50,0.55 (wake nudge)");
                    self.mark_stale(now, 900);
                } else {
                    self.note(&format!(
                        "DECIDE swipe-unlock try={} reason=waiting for lock pad",
                        self.unlock_tries
                    ));
                    self.swipe_unlock(tx, now);
                    return;
                }
                self.need_dump(tx, now);
                return;
            }
        }

        if dump.is_container() {
            if let Some(n) = dump.find(&self.profile) {
                self.status = format!("выбираю {}", self.profile);
                self.decide(
                    "container",
                    &format!("нашёл профиль «{}»", self.profile),
                    n,
                    now,
                );
                self.until = now + Duration::from_millis(1200);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
                self.stall = 0;
                return;
            }
            self.stall = self.stall.saturating_add(1);
            let seen = dump.profiles();
            self.note(&format!(
                "DECIDE scroll-container stall={} want={} seen=[{}]",
                self.stall,
                self.profile,
                seen.join(", ")
            ));
            if self.stall > 6 {
                self.fail(&format!("нет профиля «{}»", self.profile));
                return;
            }
            self.status = "листаю список".into();
            swipe(tx, 0.50, 0.70, 0.50, 0.38);
            self.note("ACT swipe list 0.50,0.70 → 0.50,0.38");
            self.until = now + Duration::from_millis(800);
            self.need_dump(tx, now);
            return;
        }

        if on_sb {
            if let Some(n) = dump.find_app_icon(&self.app_name) {
                self.status = format!("открываю {}", self.app_name);
                self.decide(
                    "springboard",
                    &format!("иконка «{}»", self.app_name),
                    n,
                    now,
                );
                self.until = now + Duration::from_millis(1400);
                self.need_dump(tx, now);
                self.phase = Phase::Work;
                self.page_tries = 0;
                self.stall = 0;
                return;
            }
            self.page_tries = self.page_tries.saturating_add(1);
            self.note(&format!(
                "DECIDE swipe-pages try={} want={} (not on this page)",
                self.page_tries, self.app_name
            ));
            if self.page_tries > 5 {
                self.fail(&format!("нет иконки «{}» на SpringBoard", self.app_name));
                return;
            }
            self.status = "листаю домашний экран…".into();
            // Next icon page.
            swipe(tx, 0.82, 0.55, 0.18, 0.55);
            self.note("ACT swipe page 0.82,0.55 → 0.18,0.55");
            self.until = now + Duration::from_millis(700);
            self.need_dump(tx, now);
            return;
        }

        if dump.is_pin() {
            self.note("SEE pin-text without pad yet — wait");
            self.need_dump(tx, now);
            self.until = now + Duration::from_millis(500);
            return;
        }

        if self.phase == Phase::Drive {
            self.status = "бужу / домой".into();
            self.note(&format!(
                "DECIDE wake+home reason=unknown screen phase={} stall={}",
                self.phase.name(),
                self.stall
            ));
            action(tx, SystemAction::Wake);
            action(tx, SystemAction::Home);
            self.note("ACT Wake + Home");
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
        self.note("DONE unknown non-drive screen — stop");
    }

    fn swipe_unlock(&mut self, tx: &Sender<InputFrame>, now: Instant) {
        self.status = "смахиваю блокировку".into();
        swipe(tx, 0.50, 0.92, 0.50, 0.22);
        self.note("ACT swipe unlock 0.50,0.92 → 0.50,0.22");
        self.mark_stale(now, 1400);
        self.need_dump(tx, now);
    }

    fn mark_stale(&mut self, now: Instant, wait_ms: u64) {
        self.stale_dump = true;
        self.waiting_dump = false;
        self.until = now + Duration::from_millis(wait_ms);
    }

    fn decide(&mut self, kind: &str, why: &str, n: &Node, now: Instant) {
        self.note(&format!(
            "DECIDE {kind}: {why} → «{}» @{:.3},{:.3}",
            trunc(n.text.trim(), 24),
            n.x,
            n.y
        ));
        self.queue_tap(n.x, n.y, n.text.trim(), now);
    }

    fn note(&mut self, msg: &str) {
        let t = self.started.elapsed().as_secs_f32();
        self.log.push(format!("[{t:5.1}s] {msg}"));
        if self.log.len() > LOG_CAP {
            let drop = self.log.len() - LOG_CAP;
            self.log.drain(0..drop);
        }
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
        self.stale_dump = false;
    }

    fn queue_tap(&mut self, x: f32, y: f32, label: &str, now: Instant) {
        if self.taps.is_empty() {
            self.tap_i = 0;
            self.tap_down = false;
            self.tap_at = now;
        }
        self.taps.push((x, y, label.to_string()));
    }

    fn queue_digits(&mut self, dump: &Dump, pin: &str, tag: &str, now: Instant) -> bool {
        let mut pts = Vec::new();
        let mut used_geo = false;
        for c in pin.chars() {
            if let Some(n) = dump.find_digit(c) {
                pts.push((n.x, n.y, c, false));
            } else if let Some(d) = c.to_digit(10) {
                let (x, y) = LOCK_PAD[d as usize];
                pts.push((x, y, c, true));
                used_geo = true;
            } else {
                self.note(&format!("FAIL {tag}: bad pin char '{c}'"));
                return false;
            }
        }
        let path: Vec<String> = pts
            .iter()
            .map(|(x, y, c, geo)| {
                if *geo {
                    format!("{c}@geo{x:.2},{y:.2}")
                } else {
                    format!("{c}@{x:.2},{y:.2}")
                }
            })
            .collect();
        self.note(&format!(
            "ACT {tag} digits{} {}",
            if used_geo { " (geom fallback)" } else { "" },
            path.join(" ")
        ));
        for (x, y, c, _) in pts {
            self.queue_tap(x, y, &c.to_string(), now);
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
        let (x, y, ref label) = self.taps[self.tap_i];
        if !self.tap_down {
            self.note(&format!(
                "TAP down «{}» @{:.3},{:.3} ({}/{})",
                trunc(label, 20),
                x,
                y,
                self.tap_i + 1,
                self.taps.len()
            ));
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
        self.note(&format!("FAIL {why}"));
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
