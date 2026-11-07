use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::mem::size_of;

use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, SelectObject, BITMAP,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_BIGGERSIZEOK,
    SIIGBF_ICONONLY,
};

/// Rendered at 64px so the 27px slot stays crisp on 200% displays.
const ICON_PX: u32 = 64;

fn ensure_com() {
    use std::cell::Cell;
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    DONE.with(|d| {
        if !d.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            d.set(true);
        }
    });
}

fn hash_of(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Extract (or read from disk cache) the shell icon for a parsing name,
/// returned as a `data:` URL ready for CSS.
pub fn data_url(src: &str) -> Option<String> {
    let png = cached_png(src)?;
    Some(format!("data:image/png;base64,{}", b64(&png)))
}

fn cached_png(src: &str) -> Option<Vec<u8>> {
    let file = crate::paths::icon_dir().join(format!("{:016x}.png", hash_of(src)));
    if let Ok(d) = std::fs::read(&file) {
        if d.len() > 8 {
            return Some(d);
        }
    }
    let data = extract(src)?;
    let _ = std::fs::write(&file, &data);
    Some(data)
}

fn extract(src: &str) -> Option<Vec<u8>> {
    ensure_com();
    unsafe {
        let name = HSTRING::from(src);
        let factory: IShellItemImageFactory = match SHCreateItemFromParsingName(&name, None) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[icon] parse failed for {src:?}: {e:?}");
                return None;
            }
        };
        let hbmp = match factory.GetImage(
            SIZE {
                cx: ICON_PX as i32,
                cy: ICON_PX as i32,
            },
            SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0),
        ) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[icon] GetImage failed for {src:?}: {e:?}");
                return None;
            }
        };

        let out = to_rgba(hbmp);
        let _ = DeleteObject(hbmp.into());

        let (buf, w, h) = match out {
            Some(v) => v,
            None => {
                eprintln!("[icon] rgba conversion failed for {src:?}");
                return None;
            }
        };
        match encode_png(&buf, w, h) {
            Some(v) => Some(v),
            None => {
                eprintln!("[icon] png encode failed for {src:?}");
                None
            }
        }
    }
}

unsafe fn to_rgba(hbmp: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<(Vec<u8>, u32, u32)> {
    let mut bm = BITMAP::default();
    if GetObjectW(
        hbmp.into(),
        size_of::<BITMAP>() as i32,
        Some(&mut bm as *mut _ as *mut c_void),
    ) == 0
    {
        return None;
    }
    let w = bm.bmWidth;
    let h = bm.bmHeight;
    if w <= 0 || h <= 0 {
        return None;
    }

    let hdc = CreateCompatibleDC(None);
    let old = SelectObject(hdc, hbmp.into());

    let mut bi = BITMAPINFO::default();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB.0;

    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    let got = GetDIBits(
        hdc,
        hbmp,
        0,
        h as u32,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut bi,
        DIB_RGB_COLORS,
    );

    SelectObject(hdc, old);
    let _ = DeleteDC(hdc);

    if got == 0 {
        return None;
    }

    // Some shell icons come back with a fully transparent alpha channel; in
    // that case treat the pixels as opaque rather than rendering nothing.
    let any_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2); // BGRA -> RGBA
        if !any_alpha {
            px[3] = 255;
        }
    }

    Some((buf, w as u32, h as u32))
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(out)
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for c in data.chunks(3) {
        let b0 = c[0] as u32;
        let b1 = *c.get(1).unwrap_or(&0) as u32;
        let b2 = *c.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
