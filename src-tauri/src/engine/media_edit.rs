//! Light audio/video editing on top of ffmpeg: trim, merge clips, and
//! low-res previews for formats the drawer's `<video>` element can't play.
//!
//! Argument builders are plain functions over parsed stream info, so they're
//! unit-tested without running ffmpeg.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::ffmpeg_engine::{ffmpeg, h264_args, parse_timestamp, run_ffmpeg_with, Ffmpeg, AUDIO_IN, VIDEO_IN};
use super::{ext_of, output_path, tool_command, Progress};

/// Shortest cut worth making, in seconds.
const MIN_LENGTH: f64 = 0.05;
/// Containers a precise (re-encoded) trim can write back to; others become MP4.
const PRECISE_KEEPS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm"];

// ---------------------------------------------------------------- trim

/// A cut from `start` to `end` seconds (`None`: to the end of the clip).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    pub start: f64,
    pub end: Option<f64>,
    /// Re-encode for frame-accurate video cuts instead of copying streams.
    pub precise: bool,
}

/// Seconds in id form: up to millisecond precision, no trailing zeros.
fn secs_id(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `1:05.3`, or `1:02:05.3` past an hour.
pub fn clock(v: f64) -> String {
    let tenths = (v * 10.0).round() as u64;
    let (h, m, s, t) = (tenths / 36000, tenths / 600 % 60, tenths / 10 % 60, tenths % 10);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{t}")
    } else {
        format!("{m}:{s:02}.{t}")
    }
}

impl Trim {
    /// The action id; the drawer builds the same form in editing.js.
    #[cfg(test)]
    pub fn id(&self) -> String {
        let end = self.end.map(secs_id).unwrap_or_default();
        let precise = if self.precise { ":precise" } else { "" };
        format!("trim:{}-{end}{precise}", secs_id(self.start))
    }

    /// `0:05.0–0:12.5`, used in recipe step titles ("Trim 0:05.0–0:12.5").
    pub fn label(&self) -> String {
        let end = self.end.map(clock).unwrap_or_else(|| "end".into());
        let precise = if self.precise { " (precise)" } else { "" };
        format!("{}–{end}{precise}", clock(self.start))
    }
}

/// Parses a trim id's argument: `5-12.5`, `5-` (to the end), `-30` (the
/// first 30 s), with an optional `:precise`.
pub fn parse_trim(arg: &str) -> Result<Trim, String> {
    let (times, precise) = match arg.strip_suffix(":precise") {
        Some(t) => (t, true),
        None => (arg, false),
    };
    let (a, b) = times.split_once('-').ok_or("A trim needs a start and an end, like 5-12.5")?;
    let num = |s: &str| -> Result<f64, String> {
        let v: f64 = s.trim().parse().map_err(|_| format!("'{s}' isn't a time in seconds"))?;
        if v.is_finite() && v >= 0.0 {
            Ok(v)
        } else {
            Err(format!("'{s}' isn't a time in seconds"))
        }
    };
    let start = if a.trim().is_empty() { 0.0 } else { num(a)? };
    let end = if b.trim().is_empty() { None } else { Some(num(b)?) };
    match end {
        Some(e) if e - start < MIN_LENGTH => Err("The end has to come after the start".into()),
        None if start == 0.0 => Err("Move the start or end handle to choose what to keep".into()),
        _ => Ok(Trim { start, end, precise }),
    }
}

fn is_video(input: &Path) -> bool {
    VIDEO_IN.contains(&ext_of(input).as_str())
}

/// Extension a trim writes: the input's own, except precise cuts of
/// containers we don't encode into (AVI, WMV), which become MP4.
pub fn trim_ext(arg: &str, input: &Path) -> Option<String> {
    let trim = parse_trim(arg).ok()?;
    let ext = ext_of(input);
    if trim.precise && is_video(input) && !PRECISE_KEEPS.contains(&ext.as_str()) {
        Some("mp4".into())
    } else {
        Some(ext)
    }
}

fn faststart(ext: &str) -> Vec<String> {
    if matches!(ext, "mp4" | "m4v" | "mov" | "m4a") {
        vec!["-movflags".into(), "+faststart".into()]
    } else {
        Vec::new()
    }
}

/// Video + audio encoder options for a re-encoded file in container `ext`.
fn encode_args(ff: &Ffmpeg, ext: &str) -> Result<Vec<String>, String> {
    let mut a = if ext == "webm" {
        vec![
            "-c:v".into(),
            ff.pick(&["libvpx-vp9", "libvpx"])?.into(),
            "-b:v".into(),
            "0".into(),
            "-crf".into(),
            "30".into(),
            "-c:a".into(),
            ff.pick(&["libopus", "libvorbis"])?.into(),
        ]
    } else {
        // Near-transparent quality: a trim shouldn't visibly cost anything.
        h264_args(ff, 18)?
    };
    a.extend(faststart(ext));
    Ok(a)
}

/// Input options (`-ss`, `-i`) and output options for `trim` of `input`,
/// writing a file with extension `out_ext`.
///
/// Without `precise`, streams are copied: instant and lossless, but video
/// can only start on a keyframe, so the cut may begin up to a few seconds
/// early. Audio always copies, since its packets are a few ms long.
pub(super) fn trim_args(ff: &Ffmpeg, trim: &Trim, input: &Path, out_ext: &str) -> Result<(Vec<OsString>, Vec<String>), String> {
    let mut inputs: Vec<OsString> = Vec::new();
    if trim.start > 0.0 {
        // Before -i: fast seek, and with re-encoding still frame-accurate.
        inputs.extend(["-ss".into(), secs_id(trim.start).into()]);
    }
    inputs.extend(["-i".into(), input.as_os_str().to_owned()]);

    let mut args = Vec::new();
    if let Some(end) = trim.end {
        args.extend(["-t".into(), secs_id(end - trim.start)]);
    }
    if trim.precise && is_video(input) {
        args.extend(["-map", "0:v:0", "-map", "0:a?", "-map", "0:s?", "-c:s", "copy"].map(String::from));
        args.extend(encode_args(ff, out_ext)?);
    } else {
        // -dn: data tracks (timecodes) often can't be written back.
        args.extend(["-map", "0", "-dn", "-c", "copy", "-avoid_negative_ts", "make_zero"].map(String::from));
        args.extend(faststart(out_ext));
    }
    Ok((inputs, args))
}

pub(super) fn trim(ff: &Ffmpeg, input: &Path, arg: &str, progress: Progress) -> Result<PathBuf, String> {
    let trim = parse_trim(arg)?;
    let length = probe(ff, input).ok().and_then(|p| p.duration);
    if let Some(total) = length {
        if trim.start >= total - MIN_LENGTH {
            return Err(format!("The start ({}) is past the end of this clip ({})", clock(trim.start), clock(total)));
        }
    }
    let out_ext = trim_ext(arg, input).ok_or("bad trim")?;
    let (inputs, args) = trim_args(ff, &trim, input, &out_ext)?;
    let out = output_path(input, "-trimmed", &out_ext);
    let expected = match (trim.end, length) {
        (Some(end), Some(total)) => Some(end.min(total) - trim.start),
        (Some(end), None) => Some(end - trim.start),
        (None, total) => total.map(|t| t - trim.start),
    };
    run_ffmpeg_with(ff, &inputs, &args, &out, expected, progress)?;
    Ok(out)
}

// ---------------------------------------------------------------- probing

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    pub codec: String,
    pub rate: Option<String>,
    pub layout: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub video: Option<VideoStream>,
    pub audio: Option<AudioStream>,
}

/// Reads the first video and audio stream from `ffmpeg -i`'s report.
pub fn parse_probe(report: &str) -> MediaInfo {
    let mut info = MediaInfo {
        duration: report
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("Duration: "))
            .and_then(|rest| rest.split(',').next())
            .and_then(parse_timestamp)
            .filter(|d| *d > 0.0),
        ..Default::default()
    };
    for line in report.lines().filter(|l| l.trim_start().starts_with("Stream #")) {
        let codec_after = |tag: &str| {
            let rest = line.split_once(tag)?.1;
            Some(rest.split([' ', ',']).next()?.to_string())
        };
        // Segments like "1280x720 [SAR 1:1 DAR 16:9]", "24 fps", "44100 Hz".
        let segments: Vec<&str> = line.split(", ").map(str::trim).collect();
        if info.video.is_none() && line.contains("Video: ") && !line.contains("(attached pic)") {
            let size = line
                .split([' ', ','])
                .filter(|t| !t.starts_with("0x"))
                .find_map(|t| {
                    let (w, h) = t.split_once('x')?;
                    Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
                })
                .filter(|(w, h)| *w > 0 && *h > 0);
            if let (Some(codec), Some((width, height))) = (codec_after("Video: "), size) {
                let fps = segments.iter().find_map(|s| s.strip_suffix(" fps")).map(str::to_string);
                info.video = Some(VideoStream { codec, width, height, fps });
            }
        } else if info.audio.is_none() && line.contains("Audio: ") {
            if let Some(codec) = codec_after("Audio: ") {
                let hz = segments.iter().position(|s| s.ends_with(" Hz"));
                info.audio = Some(AudioStream {
                    codec,
                    rate: hz.map(|i| segments[i].trim_end_matches(" Hz").to_string()),
                    layout: hz.and_then(|i| segments.get(i + 1)).map(|s| s.to_string()),
                });
            }
        }
    }
    info
}

fn probe(ff: &Ffmpeg, input: &Path) -> Result<MediaInfo, String> {
    // With no output file ffmpeg exits with an error, but prints the info first.
    let out = tool_command(&ff.exe)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .output()
        .map_err(|e| format!("Could not run ffmpeg: {e}"))?;
    Ok(parse_probe(&String::from_utf8_lossy(&out.stderr)))
}

// ---------------------------------------------------------------- merge

/// How a merge will run, shown in the Merge view before starting.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MergePlan {
    /// True when the clips can be joined without re-encoding.
    pub copy: bool,
    pub width: u32,
    pub height: u32,
    /// Extension of the merged file.
    pub ext: String,
    /// Why the clips have to be re-encoded, if they do.
    pub reason: Option<String>,
}

fn same_container(a: &str, b: &str) -> bool {
    let norm = |e: &str| if e == "m4v" { "mp4".to_string() } else { e.to_string() };
    norm(a) == norm(b)
}

fn even(v: u32) -> u32 {
    (v / 2 * 2).max(2)
}

/// Decides between the concat demuxer (same container, codecs, size, frame
/// rate and audio layout) and re-encoding to the first clip's size.
pub fn plan_merge(clips: &[(String, MediaInfo)]) -> Result<MergePlan, String> {
    let (first_ext, first) = clips.first().ok_or("Drop two or more videos to merge")?;
    let v0 = first.video.as_ref().ok_or("The first clip has no video")?;
    let mut reason = None;
    for (i, (ext, info)) in clips.iter().enumerate().skip(1) {
        let n = i + 1;
        let v = info.video.as_ref().ok_or_else(|| format!("Clip {n} has no video"))?;
        let why = if !same_container(ext, first_ext) {
            Some(format!("clip {n} is .{ext}, not .{first_ext}"))
        } else if v.codec != v0.codec {
            Some(format!("clip {n} uses {}, not {}", v.codec, v0.codec))
        } else if (v.width, v.height) != (v0.width, v0.height) {
            Some(format!("clip {n} is {}×{}, not {}×{}", v.width, v.height, v0.width, v0.height))
        } else if v.fps != v0.fps {
            Some(format!("clip {n} has a different frame rate"))
        } else if info.audio != first.audio {
            Some(format!("clip {n} has different audio"))
        } else {
            None
        };
        if why.is_some() {
            reason = why;
            break;
        }
    }
    Ok(match reason {
        None => MergePlan { copy: true, width: v0.width, height: v0.height, ext: first_ext.clone(), reason: None },
        Some(r) => MergePlan { copy: false, width: even(v0.width), height: even(v0.height), ext: "mp4".into(), reason: Some(r) },
    })
}

/// The concat demuxer's list file for `paths`.
pub fn concat_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            // Inside single quotes only the quote itself needs escaping: ' -> '\''
            let s = p.to_string_lossy().replace('\\', "/").replace('\'', r"'\''");
            format!("file '{s}'\n")
        })
        .collect()
}

/// Filter graph that scales and pads every clip to `width`x`height` at the
/// first clip's frame rate and joins them. `audio[i]` is the input index of
/// clip i's audio (its own, or generated silence); `None` when no clip has
/// sound.
pub fn merge_filter(count: usize, width: u32, height: u32, fps: &str, audio: Option<&[usize]>) -> String {
    let mut graph = String::new();
    let mut joined = String::new();
    for i in 0..count {
        graph.push_str(&format!(
            "[{i}:v:0]scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p[v{i}];"
        ));
        joined.push_str(&format!("[v{i}]"));
        if let Some(audio) = audio {
            graph.push_str(&format!(
                "[{}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}];",
                audio[i]
            ));
            joined.push_str(&format!("[a{i}]"));
        }
    }
    let a = if audio.is_some() { 1 } else { 0 };
    let outs = if audio.is_some() { "[v][a]" } else { "[v]" };
    format!("{graph}{joined}concat=n={count}:v=1:a={a}{outs}")
}

/// Input and output options to re-encode `clips` (path, info) into one MP4.
pub(super) fn merge_reencode_args(
    ff: &Ffmpeg,
    clips: &[(PathBuf, MediaInfo)],
    plan: &MergePlan,
) -> Result<(Vec<OsString>, Vec<String>), String> {
    let mut inputs: Vec<OsString> = Vec::new();
    for (path, _) in clips {
        inputs.extend(["-i".into(), path.as_os_str().to_owned()]);
    }
    let any_audio = clips.iter().any(|(_, i)| i.audio.is_some());
    let audio_map = any_audio.then(|| {
        let mut next = clips.len();
        clips
            .iter()
            .enumerate()
            .map(|(i, (_, info))| {
                if info.audio.is_some() {
                    i
                } else {
                    // A silent track as long as the clip keeps sound in sync.
                    let len = info.duration.unwrap_or(1.0);
                    inputs.extend([
                        "-f".into(),
                        "lavfi".into(),
                        "-t".into(),
                        secs_id(len).into(),
                        "-i".into(),
                        "anullsrc=r=48000:cl=stereo".into(),
                    ]);
                    next += 1;
                    next - 1
                }
            })
            .collect::<Vec<_>>()
    });
    let fps = clips[0].1.video.as_ref().and_then(|v| v.fps.clone()).unwrap_or_else(|| "30".into());
    let filter = merge_filter(clips.len(), plan.width, plan.height, &fps, audio_map.as_deref());
    let mut args = vec!["-filter_complex".to_string(), filter, "-map".into(), "[v]".into()];
    if any_audio {
        args.extend(["-map".into(), "[a]".into()]);
    }
    let mut enc = h264_args(ff, 20)?;
    if !any_audio {
        // h264_args always adds an audio encoder; drop it for silent output.
        if let Some(i) = enc.iter().position(|a| a == "-c:a") {
            enc.truncate(i);
        }
    }
    args.extend(enc);
    args.extend(faststart("mp4"));
    Ok((inputs, args))
}

fn probe_all(ff: &Ffmpeg, inputs: &[PathBuf]) -> Result<Vec<(PathBuf, MediaInfo)>, String> {
    inputs
        .iter()
        .map(|p| {
            if !is_video(p) {
                return Err(format!("{} isn't a video", p.display()));
            }
            Ok((p.clone(), probe(ff, p)?))
        })
        .collect()
}

fn plan_for(clips: &[(PathBuf, MediaInfo)]) -> Result<MergePlan, String> {
    let keyed: Vec<_> = clips.iter().map(|(p, i)| (ext_of(p), i.clone())).collect();
    plan_merge(&keyed)
}

/// What `merge` will do with `inputs`, for the Merge view.
pub fn merge_plan(inputs: &[PathBuf]) -> Result<MergePlan, String> {
    let ff = ffmpeg()?;
    plan_for(&probe_all(ff, inputs)?)
}

/// Joins `inputs` in order into `<first>-merged.<ext>`.
pub(super) fn merge(ff: &Ffmpeg, inputs: &[PathBuf], progress: Progress) -> Result<PathBuf, String> {
    if inputs.len() < 2 {
        return Err("Drop two or more videos to merge".into());
    }
    let clips = probe_all(ff, inputs)?;
    let plan = plan_for(&clips)?;
    let total: Option<f64> = clips.iter().map(|(_, i)| i.duration).sum();
    let out = output_path(&inputs[0], "-merged", &plan.ext);
    if !plan.copy {
        let (ins, args) = merge_reencode_args(ff, &clips, &plan)?;
        run_ffmpeg_with(ff, &ins, &args, &out, total, progress)?;
        return Ok(out);
    }
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let list = std::env::temp_dir().join(format!("ferryfile-concat-{}-{n}.txt", std::process::id()));
    std::fs::write(&list, concat_list(inputs)).map_err(|e| e.to_string())?;
    let ins: Vec<OsString> = vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.as_os_str().to_owned()];
    let mut args: Vec<String> = ["-map", "0", "-dn", "-c", "copy"].map(String::from).to_vec();
    args.extend(faststart(&plan.ext));
    let result = run_ffmpeg_with(ff, &ins, &args, &out, total, progress);
    let _ = std::fs::remove_file(&list);
    result.map(|_| out)
}

// ---------------------------------------------------------------- preview

/// Folder for converted previews; only the latest one is kept.
fn preview_dir() -> PathBuf {
    std::env::temp_dir().join("ferryfile-preview")
}

/// Options for a small, quick-to-make preview the drawer can play: 480p
/// H.264 + AAC MP4 for video, AAC M4A for audio.
pub(super) fn preview_args(ff: &Ffmpeg, video: bool) -> Result<Vec<String>, String> {
    let aac = ff.pick(&["aac"])?;
    if !video {
        return Ok(["-vn", "-c:a", aac, "-b:a", "128k"].map(String::from).to_vec());
    }
    let enc = ff.h264()?;
    let mut a: Vec<String> = ["-map", "0:v:0", "-map", "0:a:0?", "-vf", "scale=-2:'min(480,ih)'", "-c:v", enc, "-pix_fmt", "yuv420p"]
        .map(String::from)
        .to_vec();
    if enc == "libx264" {
        a.extend(["-preset", "veryfast", "-crf", "28"].map(String::from));
    } else {
        a.extend(["-b:v", "1200k"].map(String::from));
    }
    a.extend(["-c:a", aac, "-b:a", "96k", "-ac", "2", "-movflags", "+faststart"].map(String::from));
    Ok(a)
}

/// Converts `input` into a preview the drawer can play and returns its path.
pub fn make_preview(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let ff = ffmpeg()?;
    let video = is_video(input);
    if !video && !AUDIO_IN.contains(&ext_of(input).as_str()) {
        return Err("Only audio and video files have a preview".into());
    }
    let dir = preview_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(if video { "preview.mp4" } else { "preview.m4a" });
    let inputs = [OsString::from("-i"), input.as_os_str().to_owned()];
    run_ffmpeg_with(ff, &inputs, &preview_args(ff, video)?, &out, None, progress)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn fake_ff(encoders: &[&str]) -> Ffmpeg {
        Ffmpeg { exe: "ffmpeg".into(), encoders: encoders.iter().map(|s| s.to_string()).collect::<HashSet<_>>() }
    }

    fn strs(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn parses_trims() {
        assert_eq!(parse_trim("5-12.5").unwrap(), Trim { start: 5.0, end: Some(12.5), precise: false });
        assert_eq!(parse_trim("5-:precise").unwrap(), Trim { start: 5.0, end: None, precise: true });
        assert_eq!(parse_trim("-30").unwrap(), Trim { start: 0.0, end: Some(30.0), precise: false });
        assert!(parse_trim("12-5").is_err());
        assert!(parse_trim("5-5.01").is_err(), "too short");
        assert!(parse_trim("0-").is_err(), "keeps everything");
        assert!(parse_trim("-").is_err());
        assert!(parse_trim("a-3").is_err());
        assert!(parse_trim("NaN-3").is_err());
        assert!(parse_trim("5").is_err());

        let t = Trim { start: 1.5, end: Some(62.25), precise: true };
        assert_eq!(t.id(), "trim:1.5-62.25:precise");
        assert_eq!(parse_trim(t.id().strip_prefix("trim:").unwrap()).unwrap(), t);
        assert_eq!(t.label(), "0:01.5–1:02.3 (precise)");
        assert_eq!(Trim { start: 3725.0, end: None, precise: false }.label(), "1:02:05.0–end");
    }

    #[test]
    fn trim_output_formats() {
        assert_eq!(trim_ext("1-2", Path::new("a.avi")).as_deref(), Some("avi"));
        assert_eq!(trim_ext("1-2:precise", Path::new("a.avi")).as_deref(), Some("mp4"));
        assert_eq!(trim_ext("1-2:precise", Path::new("a.webm")).as_deref(), Some("webm"));
        assert_eq!(trim_ext("1-2:precise", Path::new("a.wav")).as_deref(), Some("wav"));
        assert_eq!(trim_ext("2-1", Path::new("a.wav")), None);
    }

    #[test]
    fn trim_copy_args() {
        let ff = fake_ff(&["aac", "libx264"]);
        let t = parse_trim("5-12.5").unwrap();
        let (ins, args) = trim_args(&ff, &t, Path::new("C:/v/clip.mp4"), "mp4").unwrap();
        assert_eq!(strs(&ins), ["-ss", "5", "-i", "C:/v/clip.mp4"]);
        assert_eq!(
            args,
            ["-t", "7.5", "-map", "0", "-dn", "-c", "copy", "-avoid_negative_ts", "make_zero", "-movflags", "+faststart"]
        );
        // From the very start: no seek. To the end: no duration.
        let (ins, args) = trim_args(&ff, &parse_trim("-3").unwrap(), Path::new("a.mkv"), "mkv").unwrap();
        assert_eq!(strs(&ins), ["-i", "a.mkv"]);
        assert_eq!(args[..2], ["-t", "3"]);
        assert!(!args.contains(&"-movflags".to_string()));
        let (_, args) = trim_args(&ff, &parse_trim("4-").unwrap(), Path::new("a.mkv"), "mkv").unwrap();
        assert!(!args.contains(&"-t".to_string()));
    }

    #[test]
    fn trim_precise_args() {
        let ff = fake_ff(&["aac", "libx264", "libvpx-vp9", "libopus"]);
        let t = parse_trim("1.25-2:precise").unwrap();
        let (ins, args) = trim_args(&ff, &t, Path::new("a.mov"), "mov").unwrap();
        assert_eq!(strs(&ins), ["-ss", "1.25", "-i", "a.mov"]);
        let joined = args.join(" ");
        assert!(joined.starts_with("-t 0.75 -map 0:v:0 -map 0:a? -map 0:s? -c:s copy -c:v libx264"), "{joined}");
        assert!(joined.contains("-crf 18") && joined.ends_with("-movflags +faststart"), "{joined}");
        assert!(!joined.contains("-c copy"));
        let (_, webm) = trim_args(&ff, &t, Path::new("a.webm"), "webm").unwrap();
        assert!(webm.join(" ").contains("-c:v libvpx-vp9") && webm.join(" ").contains("-c:a libopus"));
        // Audio copies even when "precise" is set (there's no keyframe problem).
        let (_, audio) = trim_args(&ff, &t, Path::new("a.mp3"), "mp3").unwrap();
        assert!(audio.join(" ").contains("-c copy"));
        // Without x264, H.264 still works through Media Foundation.
        let mf = fake_ff(&["aac", "h264_mf"]);
        let (_, args) = trim_args(&mf, &t, Path::new("a.mp4"), "mp4").unwrap();
        assert!(args.join(" ").contains("-c:v h264_mf"));
    }

    const REPORT: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mp4':
  Duration: 00:00:10.50, start: 0.000000, bitrate: 1234 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1280x720 [SAR 1:1 DAR 16:9], 1205 kb/s, 24 fps, 24 tbr, 12288 tbn (default)
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 128 kb/s (default)
  Stream #0:2[0x3](und): Video: mjpeg (Baseline), yuvj420p(pc), 300x300, 90k tbr (attached pic)";

    #[test]
    fn parses_probe_reports() {
        let info = parse_probe(REPORT);
        assert_eq!(info.duration, Some(10.5));
        assert_eq!(
            info.video,
            Some(VideoStream { codec: "h264".into(), width: 1280, height: 720, fps: Some("24".into()) })
        );
        assert_eq!(
            info.audio,
            Some(AudioStream { codec: "aac".into(), rate: Some("44100".into()), layout: Some("stereo".into()) })
        );
        let silent = parse_probe("  Duration: 00:00:02.00, start\n  Stream #0:0: Video: mpeg4 (Simple Profile), yuv420p, 640x360 [SAR 1:1 DAR 16:9], 29.97 fps");
        assert_eq!(silent.video.unwrap().fps.as_deref(), Some("29.97"));
        assert!(silent.audio.is_none());
    }

    fn clip(codec: &str, w: u32, h: u32, audio: bool) -> MediaInfo {
        MediaInfo {
            duration: Some(2.0),
            video: Some(VideoStream { codec: codec.into(), width: w, height: h, fps: Some("24".into()) }),
            audio: audio.then(|| AudioStream { codec: "aac".into(), rate: Some("48000".into()), layout: Some("stereo".into()) }),
        }
    }

    #[test]
    fn merge_plans() {
        let same = [("mp4".to_string(), clip("h264", 1280, 720, true)), ("m4v".to_string(), clip("h264", 1280, 720, true))];
        let plan = plan_merge(&same).unwrap();
        assert!(plan.copy, "{plan:?}");
        assert_eq!(plan.ext, "mp4");

        let sized = [("mp4".to_string(), clip("h264", 1279, 721, true)), ("mp4".to_string(), clip("h264", 1920, 1080, true))];
        let plan = plan_merge(&sized).unwrap();
        assert!(!plan.copy);
        assert_eq!((plan.width, plan.height), (1278, 720), "first clip's size, made even");
        assert_eq!(plan.reason.as_deref(), Some("clip 2 is 1920×1080, not 1279×721"));

        let codec = [("mov".to_string(), clip("h264", 640, 360, true)), ("mov".to_string(), clip("hevc", 640, 360, true))];
        assert!(plan_merge(&codec).unwrap().reason.unwrap().contains("hevc"));
        let container = [("mp4".to_string(), clip("h264", 640, 360, true)), ("mkv".to_string(), clip("h264", 640, 360, true))];
        assert!(!plan_merge(&container).unwrap().copy);
        let sound = [("mp4".to_string(), clip("h264", 640, 360, true)), ("mp4".to_string(), clip("h264", 640, 360, false))];
        assert_eq!(plan_merge(&sound).unwrap().reason.as_deref(), Some("clip 2 has different audio"));
    }

    #[test]
    fn concat_list_escapes_quotes() {
        let list = concat_list(&[PathBuf::from(r"C:\Clips\it's here.mp4"), PathBuf::from("D:/b.mp4")]);
        assert_eq!(list, "file 'C:/Clips/it'\\''s here.mp4'\nfile 'D:/b.mp4'\n");
    }

    #[test]
    fn merge_reencode_builds_filter() {
        let ff = fake_ff(&["aac", "libx264"]);
        let clips = vec![
            (PathBuf::from("a.mp4"), clip("h264", 1280, 720, true)),
            (PathBuf::from("b.mov"), clip("hevc", 1920, 1080, false)),
        ];
        let keyed: Vec<_> = clips.iter().map(|(p, i)| (ext_of(p), i.clone())).collect();
        let plan = plan_merge(&keyed).unwrap();
        let (ins, args) = merge_reencode_args(&ff, &clips, &plan).unwrap();
        // b.mov has no audio, so a silent track (input 2) stands in for it.
        assert_eq!(strs(&ins), ["-i", "a.mp4", "-i", "b.mov", "-f", "lavfi", "-t", "2", "-i", "anullsrc=r=48000:cl=stereo"]);
        let filter = &args[1];
        assert!(filter.starts_with("[0:v:0]scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:"), "{filter}");
        assert!(filter.contains("[1:v:0]scale=1280:720") && filter.contains("fps=24"), "{filter}");
        assert!(filter.contains("[2:a:0]aresample=48000"), "{filter}");
        assert!(filter.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"), "{filter}");
        assert_eq!(args[2..6], ["-map", "[v]", "-map", "[a]"]);
        assert!(args.join(" ").contains("-c:v libx264") && args.join(" ").contains("-c:a aac"));

        // No clip has sound: no audio at all.
        let mute = vec![
            (PathBuf::from("a.mp4"), clip("h264", 640, 360, false)),
            (PathBuf::from("b.mp4"), clip("h264", 320, 180, false)),
        ];
        let keyed: Vec<_> = mute.iter().map(|(p, i)| (ext_of(p), i.clone())).collect();
        let (ins, args) = merge_reencode_args(&ff, &mute, &plan_merge(&keyed).unwrap()).unwrap();
        assert_eq!(ins.len(), 4);
        assert!(args[1].ends_with("[v0][v1]concat=n=2:v=1:a=0[v]"), "{}", args[1]);
        assert!(!args.iter().any(|a| a == "-c:a" || a == "[a]"));
    }

    #[test]
    fn preview_uses_playable_codecs() {
        let ff = fake_ff(&["aac", "h264_mf"]);
        let v = preview_args(&ff, true).unwrap().join(" ");
        assert!(v.contains("-c:v h264_mf") && v.contains("-b:v 1200k") && v.contains("-c:a aac"), "{v}");
        assert_eq!(preview_args(&ff, false).unwrap().join(" "), "-vn -c:a aac -b:a 128k");
    }

    /// Trims, merges and previews generated clips. Needs ffmpeg, so opt-in:
    /// `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn edits_real_media() {
        let ff = ffmpeg().expect("ffmpeg available");
        let dir = std::env::temp_dir().join(format!("ferryfile-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gen = |name: &str, args: &[&str]| {
            let out = dir.join(name);
            let ok = tool_command(&ff.exe)
                .args(["-hide_banner", "-loglevel", "error", "-y"])
                .args(args)
                .arg(&out)
                .status()
                .unwrap()
                .success();
            assert!(ok, "failed to generate {name}");
            out
        };
        let tone = |d: &str| format!("sine=frequency=440:duration={d}");
        let a = gen("a.mp4", &["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24:duration=4", "-f", "lavfi", "-i", &tone("4"), "-c:v", "mpeg4", "-g", "24", "-c:a", "aac", "-shortest"]);
        let b = gen("b.mp4", &["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24:duration=3", "-f", "lavfi", "-i", &tone("3"), "-c:v", "mpeg4", "-g", "24", "-c:a", "aac", "-shortest"]);
        let c = gen("c.mov", &["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=2", "-c:v", "mpeg4"]);
        let wav = gen("tone.wav", &["-f", "lavfi", "-i", &tone("5")]);
        let length = |p: &Path| probe(ff, p).unwrap().duration.unwrap();

        let noop = |_: f32| {};
        let copy = trim(ff, &a, "1-3", &noop).unwrap();
        assert_eq!(ext_of(&copy), "mp4");
        let l = length(&copy);
        assert!((1.9..=3.1).contains(&l), "copy trim lasts {l}");
        let precise = trim(ff, &a, "1.5-2.5:precise", &noop).unwrap();
        let l = length(&precise);
        assert!((0.95..=1.1).contains(&l), "precise trim lasts {l}");
        let audio = trim(ff, &wav, "1-", &noop).unwrap();
        assert!((3.9..=4.1).contains(&length(&audio)));
        assert!(trim(ff, &wav, "9-12", &noop).unwrap_err().contains("past the end"));

        // Same format: concat demuxer, no re-encode.
        assert!(merge_plan(&[a.clone(), b.clone()]).unwrap().copy);
        let joined = merge(ff, &[a.clone(), b.clone()], &noop).unwrap();
        assert!((6.8..=7.2).contains(&length(&joined)), "{}", length(&joined));
        // Different size and container, one without sound: re-encoded.
        let plan = merge_plan(&[a.clone(), c.clone()]).unwrap();
        assert!(!plan.copy && plan.ext == "mp4");
        let mixed = merge(ff, &[a.clone(), c.clone()], &noop).unwrap();
        let info = probe(ff, &mixed).unwrap();
        assert!((5.8..=6.3).contains(&info.duration.unwrap()), "{info:?}");
        let v = info.video.unwrap();
        assert_eq!((v.width, v.height), (640, 360));
        assert!(info.audio.is_some());

        let preview = make_preview(&c, &noop).unwrap();
        assert!(probe(ff, &preview).unwrap().video.unwrap().codec.starts_with("h264"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
