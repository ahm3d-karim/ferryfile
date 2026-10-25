//! Values inside action ids, including the ones people type in themselves.
//!
//! Fixed choices keep their old ids (`resize:50`, `compress:75`, `fitkb:500`,
//! `scale:720`). Custom values extend the same scheme:
//!
//! | id                  | meaning                                        |
//! |---------------------|------------------------------------------------|
//! | `resize:62`         | 62% of the original size                       |
//! | `resize:w1280`      | 1280 px wide, height follows the aspect ratio  |
//! | `resize:h720`       | 720 px tall, width follows the aspect ratio    |
//! | `resize:w1280h720`  | fits inside 1280 × 720, keeping the aspect     |
//! | `resize:1280x720`   | exactly 1280 × 720 (may stretch)               |
//! | `compress:q62`      | JPEG quality 62 (`compress:62` also works)     |
//! | `fitkb:350`         | at most 350 KB (1 KB = 1000 bytes, like 1 MB)  |
//! | `scale:900`         | video at most 900 px tall                      |

use super::Action;

/// Largest side a resize may produce, in pixels.
pub const MAX_SIDE: u32 = 20_000;
pub const MAX_PERCENT: u32 = 400;
/// Video heights: 144p up to 8K.
pub const MIN_HEIGHT: u32 = 144;
pub const MAX_HEIGHT: u32 = 4320;
/// "Shrink to" limits in KB: 10 KB up to 10 GB.
pub const MIN_KB: u64 = 10;
pub const MAX_KB: u64 = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    Percent(u32),
    Width(u32),
    Height(u32),
    /// Fit inside the box, keeping the aspect ratio.
    Fit(u32, u32),
    /// Exactly this size, even if it stretches the picture.
    Exact(u32, u32),
}

fn number(s: &str) -> Option<u32> {
    // Digits only: no signs, spaces or decimals sneaking into ids.
    if s.is_empty() || s.len() > 6 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn side(s: &str, what: &str) -> Result<u32, String> {
    match number(s) {
        Some(n) if (1..=MAX_SIDE).contains(&n) => Ok(n),
        _ => Err(format!("{what} must be a whole number from 1 to {MAX_SIDE} px")),
    }
}

impl Resize {
    pub fn parse(arg: &str) -> Result<Self, String> {
        if let Some(pct) = number(arg) {
            return if (1..=MAX_PERCENT).contains(&pct) {
                Ok(Resize::Percent(pct))
            } else {
                Err(format!("Percent must be from 1 to {MAX_PERCENT}"))
            };
        }
        if let Some((w, h)) = arg.split_once('x') {
            return Ok(Resize::Exact(side(w, "Width")?, side(h, "Height")?));
        }
        if let Some(rest) = arg.strip_prefix('w') {
            return match rest.split_once('h') {
                Some((w, h)) => Ok(Resize::Fit(side(w, "Width")?, side(h, "Height")?)),
                None => Ok(Resize::Width(side(rest, "Width")?)),
            };
        }
        if let Some(h) = arg.strip_prefix('h') {
            return Ok(Resize::Height(side(h, "Height")?));
        }
        Err(format!("'{arg}' isn't a size Ferryfile knows"))
    }

    /// The new size for a `w` × `h` picture.
    pub fn apply(self, w: u32, h: u32) -> Result<(u32, u32), String> {
        let (w, h) = (w.max(1) as u64, h.max(1) as u64);
        let scaled = |num: u64, den: u64, v: u64| ((v * num + den / 2) / den).max(1);
        let (nw, nh) = match self {
            Resize::Percent(p) => (scaled(p as u64, 100, w), scaled(p as u64, 100, h)),
            Resize::Width(nw) => (nw as u64, scaled(nw as u64, w, h)),
            Resize::Height(nh) => (scaled(nh as u64, h, w), nh as u64),
            Resize::Fit(bw, bh) => {
                // Whichever side hits the box first decides the scale.
                if w * bh as u64 >= h * bw as u64 {
                    (bw as u64, scaled(bw as u64, w, h))
                } else {
                    (scaled(bh as u64, h, w), bh as u64)
                }
            }
            Resize::Exact(nw, nh) => (nw as u64, nh as u64),
        };
        if nw > MAX_SIDE as u64 || nh > MAX_SIDE as u64 {
            return Err(format!("That would be {nw} × {nh} px. The limit is {MAX_SIDE} px a side"));
        }
        Ok((nw as u32, nh as u32))
    }

    pub fn label(self) -> String {
        match self {
            Resize::Percent(p) => format!("{p}%"),
            Resize::Width(w) => format!("{w} px wide"),
            Resize::Height(h) => format!("{h} px tall"),
            Resize::Fit(w, h) => format!("to fit {w} × {h}"),
            Resize::Exact(w, h) => format!("to exactly {w} × {h}"),
        }
    }

    /// Added to the output name, e.g. `photo-w1280.png`.
    pub fn suffix(self) -> String {
        match self {
            Resize::Percent(p) => format!("-{p}pct"),
            Resize::Width(w) => format!("-w{w}"),
            Resize::Height(h) => format!("-h{h}"),
            Resize::Fit(w, h) | Resize::Exact(w, h) => format!("-{w}x{h}"),
        }
    }
}

/// JPEG quality from `q62` or the older `62`.
pub fn quality(arg: &str) -> Result<u8, String> {
    match number(arg.strip_prefix('q').unwrap_or(arg)) {
        Some(q) if (1..=100).contains(&q) => Ok(q as u8),
        _ => Err("Quality must be from 1 to 100".into()),
    }
}

/// A "Shrink to" target in KB.
pub fn target_kb(arg: &str) -> Result<u64, String> {
    match number(arg).map(u64::from) {
        Some(kb) if (MIN_KB..=MAX_KB).contains(&kb) => Ok(kb),
        _ => Err(format!("Size must be from {MIN_KB} KB to {} GB", MAX_KB / 1_000_000)),
    }
}

/// Maximum video height in px.
pub fn video_height(arg: &str) -> Result<u32, String> {
    match number(arg) {
        Some(h) if (MIN_HEIGHT..=MAX_HEIGHT).contains(&h) => Ok(h),
        _ => Err(format!("Height must be from {MIN_HEIGHT} to {MAX_HEIGHT} px")),
    }
}

/// `500 KB`, `1 MB`, `2.5 MB`.
pub fn kb_label(kb: u64) -> String {
    if kb < 1000 {
        return format!("{kb} KB");
    }
    let mb = format!("{:.2}", kb as f64 / 1000.0);
    format!("{} MB", mb.trim_end_matches('0').trim_end_matches('.'))
}

/// A "Custom…" entry the drawer can show for a group.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CustomForm {
    /// The group it belongs to, e.g. "Resize".
    pub group: String,
    /// Which form to show: `image-size`, `quality`, `target-size`, `video-height`.
    pub kind: String,
}

impl CustomForm {
    pub fn new(group: &str, kind: &str) -> Self {
        Self { group: group.into(), kind: kind.into() }
    }
}

/// The action for a custom image id, or `None` if it isn't one of these kinds.
pub fn image_action(action_id: &str) -> Option<Result<Action, String>> {
    let (kind, arg) = action_id.split_once(':')?;
    let made = |label: String, group: &str| Action { id: action_id.into(), label, group: group.into() };
    Some(match kind {
        "resize" => Resize::parse(arg).map(|r| made(r.label(), "Resize")),
        "compress" => quality(arg).map(|q| made(format!("JPEG, quality {q}"), "Compress")),
        "fitkb" => target_kb(arg).map(|kb| made(kb_label(kb), "Shrink to")),
        _ => return None,
    })
}

/// The action for a custom video id, or `None` if it isn't one of these kinds.
pub fn video_action(action_id: &str) -> Option<Result<Action, String>> {
    let (kind, arg) = action_id.split_once(':')?;
    let made = |label: String, group: &str| Action { id: action_id.into(), label, group: group.into() };
    Some(match kind {
        "scale" => video_height(arg).map(|h| made(format!("{h}p"), "Resize")),
        "fitkb" => target_kb(arg).map(|kb| made(kb_label(kb), "Shrink to")),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resize_ids() {
        assert_eq!(Resize::parse("50"), Ok(Resize::Percent(50)));
        assert_eq!(Resize::parse("w1280"), Ok(Resize::Width(1280)));
        assert_eq!(Resize::parse("h720"), Ok(Resize::Height(720)));
        assert_eq!(Resize::parse("w1280h720"), Ok(Resize::Fit(1280, 720)));
        assert_eq!(Resize::parse("1280x720"), Ok(Resize::Exact(1280, 720)));
        for bad in ["", "0", "401", "w0", "w", "h", "w-5", "w12.5", "x720", "1280x", "w1h", "abc", "w99999999", " 50", "+50", "q50"] {
            assert!(Resize::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn resize_math() {
        let size = |id: &str, w, h| Resize::parse(id).unwrap().apply(w, h).unwrap();
        assert_eq!(size("50", 41, 20), (21, 10));
        assert_eq!(size("w1280", 4000, 3000), (1280, 960));
        assert_eq!(size("h720", 4000, 3000), (960, 720));
        // A wide picture is limited by the box width, a tall one by its height.
        assert_eq!(size("w1000h1000", 4000, 2000), (1000, 500));
        assert_eq!(size("w1000h1000", 2000, 4000), (500, 1000));
        assert_eq!(size("300x100", 4000, 3000), (300, 100));
        assert_eq!(size("1", 10, 10), (1, 1));
        // Too big comes back as an error, not a giant allocation.
        assert!(Resize::parse("400").unwrap().apply(10_000, 10).is_err());
        assert!(Resize::parse("w20000").unwrap().apply(10, 100).is_err());
    }

    #[test]
    fn parses_other_values() {
        assert_eq!(quality("q62"), Ok(62));
        assert_eq!(quality("75"), Ok(75)); // old ids keep working
        for bad in ["q0", "q101", "q", "qq5", "-1", "q6.5"] {
            assert!(quality(bad).is_err(), "{bad:?}");
        }
        assert_eq!(target_kb("350"), Ok(350));
        assert_eq!(target_kb("1000"), Ok(1000));
        for bad in ["9", "0", "", "10000001", "1.5", "350kb"] {
            assert!(target_kb(bad).is_err(), "{bad:?}");
        }
        assert_eq!(video_height("900"), Ok(900));
        assert!(video_height("100").is_err() && video_height("5000").is_err());
    }

    #[test]
    fn labels() {
        assert_eq!(kb_label(350), "350 KB");
        assert_eq!(kb_label(1000), "1 MB");
        assert_eq!(kb_label(2500), "2.5 MB");
        assert_eq!(kb_label(1250), "1.25 MB");
        let a = image_action("resize:w1280").unwrap().unwrap();
        assert_eq!((a.title(), Resize::Width(1280).suffix()), ("Resize 1280 px wide".into(), "-w1280".into()));
        assert_eq!(image_action("compress:q62").unwrap().unwrap().title(), "Compress JPEG, quality 62");
        assert_eq!(image_action("fitkb:350").unwrap().unwrap().title(), "Shrink to 350 KB");
        assert_eq!(video_action("scale:900").unwrap().unwrap().title(), "Resize 900p");
        assert!(image_action("convert:png").is_none());
        assert!(image_action("resize:w0").unwrap().is_err());
    }
}
