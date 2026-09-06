//! Windows.Media.Ocr on a background thread. Coordinates are normalized [0, 1].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use crate::video::DecodedFrame;

#[derive(Clone, Debug)]
pub struct Hit {
    pub text: String,
    pub cx: f32,
    pub cy: f32,
}

static STARTED: AtomicBool = AtomicBool::new(false);
static REQ: Mutex<Option<(i32, i32, Vec<u8>)>> = Mutex::new(None);
static RES: Mutex<Option<Vec<Hit>>> = Mutex::new(None);

pub fn submit(frame: &DecodedFrame) {
    let Some(job) = downscale_bgra(frame) else {
        return;
    };
    if let Ok(mut g) = REQ.lock() {
        *g = Some(job);
    }
    ensure_worker();
}

pub fn take() -> Option<Vec<Hit>> {
    RES.lock().ok().and_then(|mut g| g.take())
}

pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c == 'ё' { 'е' } else { c })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

pub fn joined(hits: &[Hit]) -> String {
    fold(&hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>().join(" "))
}

fn ensure_worker() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::Builder::new()
        .name("ioscpy-ocr".into())
        .spawn(worker)
        .ok();
}

fn worker() {
    #[cfg(target_os = "windows")]
    win::run();
    #[cfg(not(target_os = "windows"))]
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn downscale_bgra(frame: &DecodedFrame) -> Option<(i32, i32, Vec<u8>)> {
    let sw = frame.width;
    let sh = frame.height;
    if sw < 8 || sh < 8 || frame.buf.len() < sw * sh {
        return None;
    }
    let dw = 480usize.min(sw);
    let dh = (sh as f32 * dw as f32 / sw as f32).round() as usize;
    if dh < 8 {
        return None;
    }
    let mut bgra = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let sy = y * sh / dh;
        let src_row = sy * sw;
        let dst_row = y * dw * 4;
        for x in 0..dw {
            let p = frame.buf[src_row + x * sw / dw];
            let i = dst_row + x * 4;
            bgra[i] = (p & 0xff) as u8;
            bgra[i + 1] = ((p >> 8) & 0xff) as u8;
            bgra[i + 2] = ((p >> 16) & 0xff) as u8;
            bgra[i + 3] = 255;
        }
    }
    Some((dw as i32, dh as i32, bgra))
}

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::{Interface, HSTRING};
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::WinRT::IMemoryBufferByteAccess;

    pub fn run() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let engine = make_engine();
        loop {
            let job = REQ.lock().ok().and_then(|mut g| g.take());
            let Some((w, h, bgra)) = job else {
                thread::sleep(Duration::from_millis(25));
                continue;
            };
            let hits = engine
                .as_ref()
                .and_then(|e| recognize(e, w, h, &bgra).ok())
                .unwrap_or_default();
            if let Ok(mut g) = RES.lock() {
                *g = Some(hits);
            }
        }
    }

    fn make_engine() -> Option<OcrEngine> {
        for tag in ["ru", "ru-RU"] {
            if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(tag)) {
                if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                    return Some(engine);
                }
            }
        }
        OcrEngine::TryCreateFromUserProfileLanguages().ok()
    }

    fn recognize(engine: &OcrEngine, w: i32, h: i32, bgra: &[u8]) -> windows::core::Result<Vec<Hit>> {
        let bitmap = software_bitmap(w, h, bgra)?;
        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let mut hits = Vec::new();
        let lines = result.Lines()?;
        let n = lines.Size()?;
        let fw = w as f32;
        let fh = h as f32;
        for i in 0..n {
            let line = lines.GetAt(i)?;
            let text = line.Text()?.to_string();
            if text.trim().is_empty() {
                continue;
            }
            let words = line.Words()?;
            let wn = words.Size()?;
            if wn == 0 {
                continue;
            }
            let mut x0 = f32::MAX;
            let mut y0 = f32::MAX;
            let mut x1 = 0.0_f32;
            let mut y1 = 0.0_f32;
            for j in 0..wn {
                let r = words.GetAt(j)?.BoundingRect()?;
                x0 = x0.min(r.X);
                y0 = y0.min(r.Y);
                x1 = x1.max(r.X + r.Width);
                y1 = y1.max(r.Y + r.Height);
                let wt = words.GetAt(j)?.Text()?.to_string();
                if wt.chars().count() == 1 && wt.chars().all(|c| c.is_ascii_digit()) {
                    hits.push(Hit {
                        text: wt,
                        cx: (r.X + r.Width * 0.5) / fw,
                        cy: (r.Y + r.Height * 0.5) / fh,
                    });
                }
            }
            hits.push(Hit {
                text,
                cx: ((x0 + x1) * 0.5) / fw,
                cy: ((y0 + y1) * 0.5) / fh,
            });
        }
        Ok(hits)
    }

    fn software_bitmap(w: i32, h: i32, bgra: &[u8]) -> windows::core::Result<SoftwareBitmap> {
        let bitmap = SoftwareBitmap::Create(BitmapPixelFormat::Bgra8, w, h)?;
        {
            let buffer = bitmap.LockBuffer(BitmapBufferAccessMode::Write)?;
            let reference = buffer.CreateReference()?;
            let access: IMemoryBufferByteAccess = Interface::cast(&reference)?;
            unsafe {
                let mut ptr = std::ptr::null_mut();
                let mut cap = 0u32;
                access.GetBuffer(&mut ptr, &mut cap)?;
                if !ptr.is_null() && cap > 0 {
                    let n = (bgra.len()).min(cap as usize);
                    std::ptr::copy_nonoverlapping(bgra.as_ptr(), ptr, n);
                }
            }
        }
        Ok(bitmap)
    }
}
