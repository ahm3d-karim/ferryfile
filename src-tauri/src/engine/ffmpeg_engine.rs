//! Audio and video actions backed by an ffmpeg binary.
//!
//! ffmpeg is found next to the app (where Tauri puts the bundled sidecar),
//! then on PATH. Encoders are picked from what the build actually supports, so
//! an LGPL build without x264 still works (H.264 falls back to Windows Media
//! Foundation via `h264_mf`).

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use super::media_edit;
use super::{ext_of, find_sidecar, output_path, tool_command, Action, Engine, Progress};

pub(super) const AUDIO_IN: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "aif", "wma"];
pub(super) const VIDEO_IN: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"];
/// Containers that commonly carry subtitle tracks.
const SUBTITLED: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm"];
/// "Shrink to" targets for video, in MB (decimal, so they also fit MiB limits).
const FIT_MB: &[u32] = &[10, 25, 50];

const AUDIO_OUT: &[(&str, &str)] = &[
    ("mp3", "MP3"),
    ("m4a", "M4A"),
    ("wav", "WAV"),
    ("flac", "FLAC"),
    ("ogg", "OGG"),
    ("opus", "Opus"),
];
const VIDEO_OUT: &[(&str, &str)] = &[
    ("mp4", "MP4"),
    ("mov", "MOV"),
    ("mkv", "MKV"),
    ("webm", "WebM"),
    ("gif", "GIF"),
];

pub struct FfmpegEngine;

pub(super) struct Ffmpeg {
    pub(super) exe: PathBuf,
    pub(super) encoders: HashSet<String>,
}

impl Ffmpeg {
    /// First encoder from `prefs` that this build has.
    pub(super) fn pick(&self, prefs: &[&'static str]) -> Result<&'static str, String> {
        prefs
            .iter()
            .copied()
            .find(|e| self.encoders.contains(*e))
            .ok_or_else(|| format!("This ffmpeg build has none of: {}", prefs.join(", ")))
    }

    pub(super) fn h264(&self) -> Result<&'static str, String> {
        self.pick(&["libx264", "h264_mf", "libopenh264", "mpeg4"])
    }
}

pub(super) fn ffmpeg() -> Result<&'static Ffmpeg, String> {
    static FFMPEG: OnceLock<Option<Ffmpeg>> = OnceLock::new();
    FFMPEG
        .get_or_init(|| {
            let exe = find_sidecar("ffmpeg", "-version")?;
            let out = tool_command(&exe).args(["-hide_banner", "-encoders"]).output().ok()?;
            // Lines look like " V....D libx264   libx264 H.264 ...".
            let encoders = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| {
                    let mut parts = l.split_whitespace();
                    let flags = parts.next()?;
                    (flags.len() == 6).then(|| parts.next().map(str::to_string)).flatten()
                })
                .collect();
            Some(Ffmpeg { exe, encoders })
        })
        .as_ref()
        .ok_or_else(|| "ffmpeg wasn't found. Run scripts/fetch-ffmpeg.ps1 or install ffmpeg on PATH.".into())
}

fn audio_args(ff: &Ffmpeg, ext: &str) -> Result<Vec<String>, String> {
    let args: Vec<&str> = match ext {
        "mp3" => vec!["-c:a", ff.pick(&["libmp3lame"])?, "-q:a", "2"],
        "m4a" => vec!["-c:a", ff.pick(&["aac"])?, "-b:a", "192k"],
        "wav" => vec!["-c:a", "pcm_s16le"],
        "flac" => vec!["-c:a", "flac"],
        "ogg" => vec!["-c:a", ff.pick(&["libvorbis", "libopus"])?, "-b:a", "192k"],
        "opus" => vec!["-c:a", ff.pick(&["libopus"])?, "-b:a", "128k"],
        other => return Err(format!("Unsupported audio target '{other}'")),
    };
    Ok(std::iter::once("-vn").chain(args).map(String::from).collect())
}

/// H.264 + AAC with an optional quality level (lower = better, x264 CRF scale).
pub(super) fn h264_args(ff: &Ffmpeg, crf: u32) -> Result<Vec<String>, String> {
    let enc = ff.h264()?;
    let mut args = vec!["-c:v".into(), enc.into(), "-pix_fmt".into(), "yuv420p".into()];
    match enc {
        "libx264" => args.extend(["-crf".into(), crf.to_string(), "-preset".into(), "medium".into()]),
        // Media Foundation / OpenH264 have no CRF; approximate with a bitrate.
        _ => {
            let kbps = match crf {
                0..=23 => 5000,
                24..=27 => 2500,
                _ => 1200,
            };
            args.extend(["-b:v".into(), format!("{kbps}k")]);
        }
    }
    args.extend(["-c:a".into(), ff.pick(&["aac"])?.into(), "-b:a".into(), "160k".into()]);
    Ok(args)
}

fn video_args(ff: &Ffmpeg, ext: &str) -> Result<Vec<String>, String> {
    Ok(match ext {
        "mp4" | "mov" => {
            let mut a = h264_args(ff, 23)?;
            a.extend(["-movflags".into(), "+faststart".into()]);
            a
        }
        "mkv" => h264_args(ff, 23)?,
        "webm" => vec![
            "-c:v".into(),
            ff.pick(&["libvpx-vp9", "libvpx"])?.into(),
            "-b:v".into(),
            "0".into(),
            "-crf".into(),
            "32".into(),
            "-c:a".into(),
            ff.pick(&["libopus", "libvorbis"])?.into(),
        ],
        "gif" => vec![
            "-vf".into(),
            "fps=12,scale=480:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse".into(),
            "-loop".into(),
            "0".into(),
        ],
        other => return Err(format!("Unsupported video target '{other}'")),
    })
}

/// What a quick `ffmpeg -i` says about a file.
struct Probe {
    duration: f64,
    has_audio: bool,
}

fn probe(ff: &Ffmpeg, input: &Path) -> Result<Probe, String> {
    // With no output file ffmpeg exits with an error, but prints the info first.
    let out = tool_command(&ff.exe)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .output()
        .map_err(|e| format!("Could not run ffmpeg: {e}"))?;
    let info = String::from_utf8_lossy(&out.stderr);
    let duration = info
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("Duration: "))
        .and_then(|rest| rest.split(',').next())
        .and_then(parse_timestamp)
        .filter(|d| *d > 0.0)
        .ok_or("Couldn't tell how long this video is")?;
    let has_audio = info.lines().any(|l| l.contains("Stream #") && l.contains("Audio:"));
    Ok(Probe { duration, has_audio })
}

/// Bitrates (kbps) that make a `duration`-second video fit in `target`
/// bytes, leaving ~8% for the container and encoder overshoot.
fn fit_bitrates(target: u64, duration: f64, has_audio: bool) -> Result<(u32, u32), String> {
    let total = target as f64 * 8.0 / 1000.0 * 0.92 / duration;
    let audio = match (has_audio, total) {
        (false, _) => 0.0,
        (true, t) if t < 500.0 => 64.0,
        _ => 128.0,
    };
    let video = total - audio;
    if video < 100.0 {
        return Err("This video is too long to fit in that size at a watchable quality".into());
    }
    Ok((video as u32, audio as u32))
}

/// Tallest frame worth keeping at a given video bitrate.
fn fit_height(video_kbps: u32) -> u32 {
    match video_kbps {
        2500.. => 1080,
        1200.. => 720,
        600.. => 480,
        _ => 360,
    }
}

/// Re-encodes `input` to an MP4 no bigger than `target` bytes. Encoders
/// overshoot now and then, so it checks the result and retries lower.
fn fit_video(ff: &Ffmpeg, input: &Path, target: u64, suffix: &str, progress: Progress) -> Result<PathBuf, String> {
    let size = std::fs::metadata(input).map_err(|e| e.to_string())?.len();
    if size <= target {
        let label = suffix.trim_start_matches('-').replace("MB", " MB");
        return Err(format!("Already under {label} — nothing to shrink"));
    }
    let info = probe(ff, input)?;
    let (mut video_kbps, audio_kbps) = fit_bitrates(target, info.duration, info.has_audio)?;
    // Hitting a size needs an encoder that honours bitrates: OpenH264 lands
    // within a few percent where Media Foundation can overshoot severalfold.
    let enc = ff.pick(&["libx264", "libopenh264", "h264_mf", "mpeg4"])?;
    for attempt in 0..3 {
        let height = fit_height(video_kbps);
        let mut args = vec![
            "-vf".to_string(),
            format!("scale=-2:'min({height},ih)'"),
            "-c:v".into(),
            enc.into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-b:v".into(),
            format!("{video_kbps}k"),
        ];
        match enc {
            "libx264" => args.extend([
                "-maxrate".into(),
                format!("{}k", video_kbps * 6 / 5),
                "-bufsize".into(),
                format!("{}k", video_kbps * 2),
            ]),
            "libopenh264" => {
                args.extend(["-rc_mode".into(), "bitrate".into()]);
                if attempt == 2 {
                    // Last try: let it drop frames rather than miss the size.
                    args.extend(["-allow_skip_frames".into(), "1".into()]);
                }
            }
            _ => {}
        }
        if audio_kbps > 0 {
            args.extend(["-c:a".into(), ff.pick(&["aac"])?.into(), "-b:a".into(), format!("{audio_kbps}k")]);
        } else {
            args.push("-an".into());
        }
        args.extend(["-movflags".into(), "+faststart".into()]);
        let out = output_path(input, suffix, "mp4");
        run_ffmpeg(ff, input, &args, &out, progress)?;
        let got = std::fs::metadata(&out).map_err(|e| e.to_string())?.len();
        if got <= target {
            return Ok(out);
        }
        let _ = std::fs::remove_file(&out);
        if attempt < 2 {
            // Aim lower by how much we missed, plus a margin.
            video_kbps = ((video_kbps as f64) * (target as f64 / got as f64) * 0.93) as u32;
            if video_kbps < 100 {
                break;
            }
        }
    }
    Err("Couldn't get this video small enough. Try a bigger size, or trim it first".into())
}

/// Parses `HH:MM:SS.xx` into seconds.
pub(super) fn parse_timestamp(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn run_ffmpeg(ff: &Ffmpeg, input: &Path, args: &[String], out: &Path, progress: Progress) -> Result<(), String> {
    let inputs = [OsString::from("-i"), input.as_os_str().to_owned()];
    run_ffmpeg_with(ff, &inputs, args, out, None, progress)
}

/// Runs ffmpeg with input options `inputs` (each `-i` plus anything before
/// it, like `-ss`) and output options `args`. Progress is measured against
/// `total` seconds of output, else the first input's duration.
pub(super) fn run_ffmpeg_with(
    ff: &Ffmpeg,
    inputs: &[OsString],
    args: &[String],
    out: &Path,
    total: Option<f64>,
    progress: Progress,
) -> Result<(), String> {
    let mut child = tool_command(&ff.exe)
        .args(["-hide_banner", "-nostdin", "-n"])
        .args(inputs)
        .args(args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(out)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;

    // stderr: grab the input duration and keep the tail for error messages.
    let duration = Arc::new(Mutex::new(total.filter(|t| *t > 0.0)));
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr = child.stderr.take().unwrap();
    let (d, t) = (duration.clone(), tail.clone());
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(rest) = line.trim_start().strip_prefix("Duration: ") {
                let mut d = d.lock().unwrap();
                if d.is_none() {
                    *d = rest.split(',').next().and_then(parse_timestamp);
                }
            }
            let mut t = t.lock().unwrap();
            t.push_back(line);
            if t.len() > 6 {
                t.pop_front();
            }
        }
    });

    // stdout: `-progress` key=value lines (several a second), also our chance
    // to stop a cancelled job.
    let stdout = child.stdout.take().unwrap();
    let mut cancelled = false;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if super::jobs::is_cancelled() {
            let _ = child.kill();
            cancelled = true;
            break;
        }
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
            if let Some(total) = *duration.lock().unwrap() {
                if total > 0.0 {
                    progress(((us / 1_000_000.0) / total).clamp(0.0, 1.0) as f32);
                }
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = err_thread.join();
    if cancelled {
        let _ = std::fs::remove_file(out);
        return Err(super::jobs::CANCELLED.into());
    }
    if status.success() {
        progress(1.0);
        Ok(())
    } else {
        let _ = std::fs::remove_file(out);
        let tail = tail.lock().unwrap();
        Err(format!(
            "ffmpeg failed: {}",
            tail.iter().rev().find(|l| !l.trim().is_empty()).map(String::as_str).unwrap_or("unknown error")
        ))
    }
}

impl Engine for FfmpegEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        let ext = ext_of(path);
        let mut out = Vec::new();
        if AUDIO_IN.contains(&ext.as_str()) {
            for (t, label) in AUDIO_OUT {
                if *t != ext {
                    out.push(Action::new(&format!("convert:{t}"), label, "Convert"));
                }
            }
        } else if VIDEO_IN.contains(&ext.as_str()) {
            for (t, label) in VIDEO_OUT {
                if *t != ext {
                    out.push(Action::new(&format!("convert:{t}"), label, "Convert"));
                }
            }
            out.push(Action::new("audio:mp3", "MP3", "Extract audio"));
            out.push(Action::new("audio:m4a", "M4A", "Extract audio"));
            out.push(Action::new("scale:1080", "1080p", "Resize"));
            out.push(Action::new("scale:720", "720p", "Resize"));
            out.push(Action::new("scale:480", "480p", "Resize"));
            out.push(Action::new("compress:28", "Smaller MP4", "Compress"));
            for mb in FIT_MB {
                out.push(Action::new(&format!("fitmb:{mb}"), &format!("{mb} MB"), "Shrink to"));
            }
            if SUBTITLED.contains(&ext.as_str()) {
                out.push(Action::new("subs:srt", "SRT", "Extract subtitles"));
            }
        }
        out
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert" | "audio", ext) => Some(ext.to_string()),
            ("scale" | "compress" | "fitmb", _) => Some("mp4".into()),
            ("subs", ext) => Some(ext.to_string()),
            ("trim", arg) => media_edit::trim_ext(arg, _input),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        let ff = ffmpeg()?;
        let (kind, arg) = action_id.split_once(':').ok_or("bad action id")?;
        let is_audio = AUDIO_IN.contains(&ext_of(input).as_str());
        let (out, args) = match kind {
            "convert" if is_audio => (output_path(input, "", arg), audio_args(ff, arg)?),
            "convert" => (output_path(input, "", arg), video_args(ff, arg)?),
            "audio" => (output_path(input, "", arg), audio_args(ff, arg)?),
            "scale" => {
                let height: u32 = arg.parse().map_err(|_| "bad height")?;
                let mut a = vec!["-vf".to_string(), format!("scale=-2:'min({height},ih)'")];
                a.extend(h264_args(ff, 23)?);
                (output_path(input, &format!("-{height}p"), "mp4"), a)
            }
            "compress" => {
                let crf: u32 = arg.parse().map_err(|_| "bad quality")?;
                (output_path(input, "-compressed", "mp4"), h264_args(ff, crf)?)
            }
            "fitmb" => {
                let mb: u64 = arg.parse().map_err(|_| "bad size")?;
                return fit_video(ff, input, mb * 1_000_000, &format!("-{mb}MB"), progress);
            }
            "subs" => {
                let args = ["-map", "0:s:0", "-c:s", "srt"].map(String::from).to_vec();
                let out = output_path(input, "", "srt");
                return match run_ffmpeg(ff, input, &args, &out, progress) {
                    Err(e) if e.contains("matches no streams") => Err("This video has no subtitle track".into()),
                    Err(e) if e.contains("not supported") || e.contains("Subtitle encoding") => Err(
                        "This video's subtitles are pictures (like DVD or Blu-ray subtitles), which can't become text".into(),
                    ),
                    other => other.map(|_| out),
                };
            }
            "trim" => return media_edit::trim(ff, input, arg, progress),
            other => return Err(format!("Unknown action kind '{other}'")),
        };
        run_ffmpeg(ff, input, &args, &out, progress)?;
        Ok(out)
    }

    /// `trim:<start>-<end>[:precise]` in seconds, from the Trim view.
    fn parse_action(&self, action_id: &str, path: &Path) -> Option<Action> {
        let ext = ext_of(path);
        if !AUDIO_IN.contains(&ext.as_str()) && !VIDEO_IN.contains(&ext.as_str()) {
            return None;
        }
        let arg = action_id.strip_prefix("trim:")?;
        let trim = media_edit::parse_trim(arg).ok()?;
        Some(Action::new(action_id, &trim.label(), "Trim"))
    }

    fn combine_actions(&self, inputs: &[PathBuf]) -> Vec<Action> {
        if inputs.len() >= 2 && inputs.iter().all(|p| VIDEO_IN.contains(&ext_of(p).as_str())) {
            vec![Action::new("combine:video", "Merge videos", "Combine")]
        } else {
            Vec::new()
        }
    }

    fn combine(&self, action_id: &str, inputs: &[PathBuf], progress: Progress) -> Result<PathBuf, String> {
        if action_id != "combine:video" {
            return Err(format!("Unknown action '{action_id}'"));
        }
        media_edit::merge(ffmpeg()?, inputs, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp("00:01:02.50"), Some(62.5));
        assert_eq!(parse_timestamp("01:00:00.00"), Some(3600.0));
        assert_eq!(parse_timestamp("N/A"), None);
    }

    /// Runs every action on generated media. Needs ffmpeg, so opt-in:
    /// `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn every_action_runs() {
        let ff = ffmpeg().expect("ffmpeg available");
        let dir = std::env::temp_dir().join(format!("ferryfile-ff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let gen = |out: &Path, args: &[&str]| {
            let ok = tool_command(&ff.exe)
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .arg(out)
                .status()
                .unwrap()
                .success();
            assert!(ok, "failed to generate {out:?}");
        };
        let subs = dir.join("subs.srt");
        std::fs::write(&subs, "1\n00:00:00,000 --> 00:00:01,500\nHello there\n\n").unwrap();
        let video = dir.join("clip.mov");
        gen(&video, &[
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=24:duration=2",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
            "-i", subs.to_str().unwrap(),
            "-map", "0:v", "-map", "1:a", "-map", "2:s",
            "-c:v", "mpeg4", "-c:a", "aac", "-c:s", "mov_text", "-shortest",
        ]);
        let audio = dir.join("tone.wav");
        gen(&audio, &["-f", "lavfi", "-i", "sine=frequency=440:duration=2"]);

        let e = FfmpegEngine;
        for input in [&video, &audio] {
            // "Shrink to" refuses files already under the size (tested below).
            for action in e.actions(input).into_iter().filter(|a| !a.id.starts_with("fitmb:")) {
                let last = std::cell::Cell::new(0.0f32);
                let out = e
                    .run(&action.id, input, &|f| last.set(f))
                    .unwrap_or_else(|err| panic!("{} on {input:?}: {err}", action.id));
                let size = std::fs::metadata(&out).unwrap().len();
                assert!(size > 0, "{} produced an empty file", action.id);
                assert_eq!(last.get(), 1.0, "{} never reported completion", action.id);
                println!("ok {:<14} -> {} ({size} bytes)", action.id, out.file_name().unwrap().to_string_lossy());
            }
        }
        let srt = std::fs::read_to_string(dir.join("clip.srt")).unwrap();
        assert!(srt.contains("Hello there"), "{srt}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fit_bitrate_math() {
        // 60 s into 10 MB: ~1227 kbps in total, 128 of it audio.
        let (v, a) = fit_bitrates(10_000_000, 60.0, true).unwrap();
        assert_eq!(a, 128);
        assert!((1090..=1110).contains(&v), "{v}");
        assert_eq!(fit_height(v), 480);
        assert_eq!(fit_bitrates(10_000_000, 60.0, false).unwrap().1, 0);
        // An hour can't fit in 10 MB.
        assert!(fit_bitrates(10_000_000, 3600.0, true).is_err());
    }

    /// Shrinks a noisy clip to a small target. Needs ffmpeg: `--ignored`.
    #[test]
    #[ignore]
    fn shrinks_video_to_target() {
        let ff = ffmpeg().expect("ffmpeg available");
        let dir = std::env::temp_dir().join(format!("ferryfile-fit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("noisy.mp4");
        let ok = tool_command(&ff.exe)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=6,noise=alls=40:allf=t"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=6"])
            .args(["-c:v", "mpeg4", "-q:v", "2", "-c:a", "aac", "-shortest"])
            .arg(&video)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let before = std::fs::metadata(&video).unwrap().len();
        let target = 1_000_000;
        assert!(before > target, "test clip too small: {before}");

        let out = fit_video(ff, &video, target, "-1MB", &|_| {}).unwrap();
        let after = std::fs::metadata(&out).unwrap().len();
        assert!(after <= target, "{after} > {target}");
        assert!(after > target / 4, "wasted the budget: {after}");
        // Already small enough: says so and writes nothing.
        let err = fit_video(ff, &out, target, "-1MB", &|_| {}).unwrap_err();
        assert_eq!(err, "Already under 1 MB — nothing to shrink");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Needs ffmpeg, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn cancel_stops_ffmpeg() {
        let ff = ffmpeg().expect("ffmpeg available");
        let dir = std::env::temp_dir().join(format!("ferryfile-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("long.mov");
        let made = tool_command(&ff.exe)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=60", "-c:v", "mpeg4"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(made.success());

        let input = video.clone();
        let worker = std::thread::spawn(move || {
            let _job = super::super::jobs::begin(Some("cancel-test"));
            FfmpegEngine.run("convert:webm", &input, &|_| {})
        });
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(super::super::jobs::cancel("cancel-test"), "job should still be running");
        let started = std::time::Instant::now();
        let result = worker.join().unwrap();
        assert_eq!(result, Err(super::super::jobs::CANCELLED.to_string()));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "stops promptly");
        assert!(!dir.join("long.webm").exists(), "partial output removed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn actions_by_type() {
        let e = FfmpegEngine;
        let a: Vec<_> = e.actions(Path::new("x.wav")).into_iter().map(|a| a.id).collect();
        assert!(a.contains(&"convert:mp3".into()) && !a.contains(&"convert:wav".into()));
        let v: Vec<_> = e.actions(Path::new("x.MOV")).into_iter().map(|a| a.id).collect();
        assert!(v.contains(&"audio:mp3".into()) && !v.contains(&"convert:mov".into()));
        assert!(e.actions(Path::new("x.png")).is_empty());
    }
}
