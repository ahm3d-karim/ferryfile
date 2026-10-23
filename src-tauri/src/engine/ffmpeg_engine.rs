//! Audio and video actions backed by an ffmpeg binary.
//!
//! ffmpeg is found next to the app (where Tauri puts the bundled sidecar),
//! then on PATH. Encoders are picked from what the build actually supports, so
//! an LGPL build without x264 still works (H.264 falls back to Windows Media
//! Foundation via `h264_mf`).

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use super::{ext_of, find_sidecar, output_path, tool_command, Action, Engine, Progress};

const AUDIO_IN: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "aif", "wma"];
const VIDEO_IN: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv"];

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

struct Ffmpeg {
    exe: PathBuf,
    encoders: HashSet<String>,
}

impl Ffmpeg {
    /// First encoder from `prefs` that this build has.
    fn pick(&self, prefs: &[&'static str]) -> Result<&'static str, String> {
        prefs
            .iter()
            .copied()
            .find(|e| self.encoders.contains(*e))
            .ok_or_else(|| format!("This ffmpeg build has none of: {}", prefs.join(", ")))
    }

    fn h264(&self) -> Result<&'static str, String> {
        self.pick(&["libx264", "h264_mf", "libopenh264", "mpeg4"])
    }
}

fn ffmpeg() -> Result<&'static Ffmpeg, String> {
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
fn h264_args(ff: &Ffmpeg, crf: u32) -> Result<Vec<String>, String> {
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

/// Parses `HH:MM:SS.xx` into seconds.
fn parse_timestamp(s: &str) -> Option<f64> {
    let mut parts = s.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn run_ffmpeg(ff: &Ffmpeg, input: &Path, args: &[String], out: &Path, progress: Progress) -> Result<(), String> {
    let mut child = tool_command(&ff.exe)
        .args(["-hide_banner", "-nostdin", "-n", "-i"])
        .arg(input)
        .args(args)
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(out)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;

    // stderr: grab the input duration and keep the tail for error messages.
    let duration = Arc::new(Mutex::new(None::<f64>));
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
        }
        out
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert" | "audio", ext) => Some(ext.to_string()),
            ("scale" | "compress", _) => Some("mp4".into()),
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
            other => return Err(format!("Unknown action kind '{other}'")),
        };
        run_ffmpeg(ff, input, &args, &out, progress)?;
        Ok(out)
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
        let video = dir.join("clip.mov");
        gen(&video, &[
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=24:duration=2",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
            "-c:v", "mpeg4", "-c:a", "aac", "-shortest",
        ]);
        let audio = dir.join("tone.wav");
        gen(&audio, &["-f", "lavfi", "-i", "sine=frequency=440:duration=2"]);

        let e = FfmpegEngine;
        for input in [&video, &audio] {
            for action in e.actions(input) {
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
