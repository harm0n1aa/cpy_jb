//! Second ioscpy host: UI-tree auto. Does not replace `ioscpy.exe`.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app_auto;
mod auto_api;
mod cli;
mod clipboard;
mod config;
mod device;
mod h264;
mod health;
mod input;
mod installer;
mod keyboard;
mod logging;
mod mouse;
mod platform;
mod protocol;
mod sidebar;
mod update;
mod usbmux;
mod video;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland_compat;
mod window;

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::cli::Cli;
use crate::input::InputFrame;
use crate::protocol::MessageType;
use crate::window::FrameSlot;

const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let cli = Cli::parse_args();
    logging::set_debug(cli.debug);
    #[cfg(all(unix, not(target_os = "macos")))]
    wayland_compat::apply_decoration_workaround(cli.wayland);
    if let Err(e) = app_auto::run(cli) {
        eprintln!("ioscpy-auto: error: {e:#}");
        std::process::exit(1);
    }
}

fn set_status(slot: &Option<Arc<Mutex<Option<String>>>>, msg: String) {
    if let Some(s) = slot {
        if let Ok(mut g) = s.lock() {
            *g = Some(msg);
        }
    }
}

fn phone_status(ack: &protocol::HelloAck) -> String {
    let c = &ack.capabilities;
    if c.keyboard {
        return "демон ответил, ждём кадр…".into();
    }
    if !c.diag_summary.is_empty() {
        return c.diag_summary.clone();
    }
    if !c.hook_dylib {
        return "твик не установлен — поставь 0.1.23 с LAN-репо".into();
    }
    if !c.inject_error.is_empty() {
        return format!("инжект: {}", c.inject_error);
    }
    if !c.hook_loaded {
        return "ElleKit не загрузил твик (arm64 в arm64e SpringBoard)".into();
    }
    "твик загрузился, но не подключился к демону".into()
}

pub(crate) fn run_connection_loop(
    cli: &Cli,
    port: u16,
    stop: &Arc<AtomicBool>,
    frame_sink: Option<FrameSlot>,
    input_rx: Option<mpsc::Receiver<InputFrame>>,
    ui_out: Option<mpsc::Sender<(MessageType, Vec<u8>)>>,
    status: Option<Arc<Mutex<Option<String>>>>,
    diag: Option<Arc<Mutex<Vec<String>>>>,
) -> Result<()> {
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        let mut forward: Option<usbmux::UsbForward> = None;
        let mut stream = match establish(cli, port, &mut forward) {
            Ok(s) => s,
            Err(e) => {
                warn!("{e:#}");
                set_status(&status, format!("USB: {e:#}"));
                if !reconnect_wait(stop) {
                    break;
                }
                continue;
            }
        };
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(8))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(8))).ok();
        let ack = match protocol::handshake(&mut stream, HOST_VERSION) {
            Ok(ack) => ack,
            Err(e) => {
                warn!("handshake failed: {e}");
                set_status(
                    &status,
                    "демон на телефоне не отвечает — ioscpy в Sileo и userspace reboot".into(),
                );
                if !reconnect_wait(stop) {
                    break;
                }
                continue;
            }
        };
        if ack.protocol_version != protocol::PROTOCOL_VERSION {
            bail!(
                "protocol mismatch (host v{}, phone v{})",
                protocol::PROTOCOL_VERSION,
                ack.protocol_version
            );
        }
        if first {
            health::print_capabilities(&ack);
        }
        first = false;
        if let Some(d) = &diag {
            if let Ok(mut g) = d.lock() {
                *g = ack.capabilities.diagnostics.clone();
            }
        }
        set_status(&status, phone_status(&ack));
        let codec = if !cli.mjpeg && ack.capabilities.stream_backends.iter().any(|b| b == "h264") {
            protocol::VIDEO_CODEC_H264
        } else {
            protocol::VIDEO_CODEC_MJPEG
        };
        match health::run_session_ex(
            stream,
            stop.clone(),
            frame_sink.clone(),
            input_rx.as_ref(),
            None,
            codec,
            false,
            ui_out.as_ref(),
            diag.as_ref(),
        )? {
            health::SessionEnd::Quit => break,
            health::SessionEnd::Lost => {
                warn!("connection lost, reconnecting…");
                if !reconnect_wait(stop) {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn establish(
    cli: &Cli,
    port: u16,
    forward_slot: &mut Option<usbmux::UsbForward>,
) -> Result<TcpStream> {
    if let Some(addr) = &cli.addr {
        return TcpStream::connect(addr).with_context(|| format!("could not connect to {addr}"));
    }
    let devices = device::list_devices()?;
    let dev = device::select_device(devices, cli.device.as_deref())?;
    info!("device {}, {} (iOS {})", dev.udid, dev.product_type, dev.ios_version);
    let forward = usbmux::UsbForward::start(&dev.udid, port)
        .context("couldn't set up the USB link to the iPhone")?;
    let stream = forward.connect()?;
    *forward_slot = Some(forward);
    Ok(stream)
}

fn reconnect_wait(stop: &Arc<AtomicBool>) -> bool {
    for _ in 0..15 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !stop.load(Ordering::Relaxed)
}
