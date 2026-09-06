//! Finds attached devices by shelling out to `idevice_id` and `ideviceinfo`.
//! A native usbmux client could replace this later without touching the API.

use std::collections::{BTreeSet, HashMap};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone)]
pub struct Device {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
}

impl Device {
    /// One-line summary used by `--list`.
    pub fn summary(&self) -> String {
        format!(
            "{}  {}  (iOS {})  \"{}\"",
            self.udid, self.product_type, self.ios_version, self.name
        )
    }

    /// Marketing name when we know this product type, otherwise the raw identifier.
    pub fn model_label(&self) -> String {
        friendly_model(&self.product_type)
            .unwrap_or(&self.product_type)
            .to_string()
    }

    pub fn short_udid(&self) -> String {
        let u = self.udid.as_str();
        if u.len() <= 12 {
            u.to_string()
        } else {
            format!("{}…{}", &u[..6], &u[u.len() - 4..])
        }
    }
}

fn friendly_model(product: &str) -> Option<&'static str> {
    Some(match product {
        "iPhone10,3" | "iPhone10,6" => "iPhone X",
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE (2nd gen)",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,6" => "iPhone SE (3rd gen)",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    })
}

fn resolve_tool(cmd: &str) -> Result<std::path::PathBuf> {
    let hint = crate::platform::missing_tools_hint();
    if let Some(p) = crate::platform::tool_path(cmd) {
        return Ok(p);
    }
    // libimobiledevice vs libimobiledevice-win32 names
    let alt = match cmd {
        "idevice_id" => Some("idevice_id"),
        "ideviceinfo" => Some("ideviceinfo"),
        _ => None,
    };
    if let Some(alt) = alt {
        if alt != cmd {
            if let Some(p) = crate::platform::tool_path(alt) {
                return Ok(p);
            }
        }
    }
    Err(anyhow!("couldn't find `{cmd}`. {hint}"))
}

fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let exe = resolve_tool(cmd)?;
    let out = crate::platform::hidden_command(&exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| {
            format!(
                "couldn't run `{cmd}`. {}",
                crate::platform::missing_tools_hint()
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        bail!(
            "`{cmd} {}` failed: {}",
            args.join(" "),
            if stderr.is_empty() { stdout } else { stderr }
        );
    }
    if stdout.is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}

fn ideviceinfo(udid: &str, key: &str) -> String {
    run("ideviceinfo", &["-u", udid, "-k", key]).unwrap_or_else(|_| "unknown".to_string())
}

fn info_cache() -> &'static Mutex<HashMap<String, Device>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Device>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fetch_info(udid: &str) -> Device {
    Device {
        name: ideviceinfo(udid, "DeviceName"),
        product_type: ideviceinfo(udid, "ProductType"),
        ios_version: ideviceinfo(udid, "ProductVersion"),
        udid: udid.to_string(),
    }
}

fn parse_udids(raw: &str) -> BTreeSet<String> {
    raw.lines()
        .map(|l| l.split_whitespace().next().unwrap_or("").to_string())
        .filter(|s| s.len() >= 8 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .collect()
}

/// List attached devices, deduped across USB and network entries.
///
/// USB presence is a single `idevice_id -l`. Name / model / iOS are cached per
/// UDID so a background poll does not spawn a console process per property.
pub fn list_devices() -> Result<Vec<Device>> {
    let raw = run("idevice_id", &["-l"]).context("couldn't check for attached iPhones")?;
    let mut udids = parse_udids(&raw);
    if udids.is_empty() {
        if let Ok(alt) = run("idevice_id", &[]) {
            udids = parse_udids(&alt);
        }
    }

    let mut cache = info_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|k, _| udids.contains(k));

    let mut devices = Vec::with_capacity(udids.len());
    for udid in udids {
        if let Some(known) = cache.get(&udid) {
            devices.push(known.clone());
        } else {
            let dev = fetch_info(&udid);
            cache.insert(udid, dev.clone());
            devices.push(dev);
        }
    }
    Ok(devices)
}

/// Pick the device to use. Honors an explicit UDID, auto-picks when there's only
/// one, otherwise asks the user to choose.
pub fn select_device(devices: Vec<Device>, requested: Option<&str>) -> Result<Device> {
    if let Some(want) = requested {
        return devices
            .into_iter()
            .find(|d| d.udid == want)
            .ok_or_else(|| anyhow!("no iPhone with UDID {want} is plugged in"));
    }
    match devices.len() {
        0 => bail!("no iPhone found over USB. Plug in your jailbroken iPhone, unlock it, and tap Trust if it asks."),
        1 => Ok(devices.into_iter().next().unwrap()),
        _ => {
            let list = devices
                .iter()
                .map(|d| format!("  {}", d.summary()))
                .collect::<Vec<_>>()
                .join("\n");
            bail!("more than one iPhone is plugged in. Pick the one you want with --device <UDID>:\n{list}")
        }
    }
}
