//! Suggested recipes. Notices when the same chain of actions keeps being run
//! on a file and then on its outputs (Resize 50%, then Convert WebP on the
//! resized copy), and after the third time offers to save it as a recipe.
//! The history stays on this machine, in `suggestions.json` next to
//! `recipes.json`, and is capped in size.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::engine::{self, Action};

/// Runs of the same chain before it's suggested.
const THRESHOLD: u32 = 3;
/// Outputs remembered as possible chain links.
const MAX_RECENT: usize = 50;
/// Distinct chains counted; the least used are forgotten first.
const MAX_CHAINS: usize = 100;
const MAX_DISMISSED: usize = 100;
/// Longer chains stop growing.
const MAX_STEPS: usize = 8;
/// Acting on an output later than this starts a new chain.
const LINK_WINDOW_SECS: u64 = 12 * 60 * 60;

/// A file an action produced, and the steps that led to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Produced {
    path: String,
    steps: Vec<String>,
    /// Extension of the file the chain started from.
    start_ext: String,
    at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ChainCount {
    steps: Vec<String>,
    start_ext: String,
    count: u32,
    last: u64,
    /// Closing a suggestion without choosing waits for this many runs before asking again.
    #[serde(default)]
    snoozed_at: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct History {
    recent: Vec<Produced>,
    chains: Vec<ChainCount>,
    dismissed: Vec<Vec<String>>,
}

/// A chain worth offering as a recipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub steps: Vec<String>,
    pub start_ext: String,
    pub count: u32,
}

/// Paths compare case-insensitively with either slash, like Windows does.
fn key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

impl History {
    /// Records that `steps` ran on `input` and made `output`. If `input` was
    /// itself made by an earlier action, the steps extend that chain.
    pub fn record(&mut self, input: &str, steps: &[String], output: &str, now: u64) {
        if steps.is_empty() {
            return;
        }
        let input_key = key(input);
        let link = self
            .recent
            .iter()
            .rev()
            .find(|p| key(&p.path) == input_key && now.saturating_sub(p.at) <= LINK_WINDOW_SECS)
            .cloned();
        let (mut chain, start_ext) = match link {
            Some(p) => (p.steps, p.start_ext),
            None => (Vec::new(), engine::ext_of(Path::new(input))),
        };
        if chain.len() + steps.len() > MAX_STEPS {
            chain.clear(); // too long to be a recipe anyone wants; start over
        }
        chain.extend_from_slice(steps);

        let output_key = key(output);
        self.recent.retain(|p| key(&p.path) != output_key);
        self.recent.push(Produced { path: output.into(), steps: chain.clone(), start_ext: start_ext.clone(), at: now });
        if self.recent.len() > MAX_RECENT {
            self.recent.drain(..self.recent.len() - MAX_RECENT);
        }

        if chain.len() < 2 {
            return;
        }
        match self.chains.iter_mut().find(|c| c.steps == chain) {
            Some(c) => {
                c.count += 1;
                c.last = now;
                c.start_ext = start_ext;
            }
            None => {
                self.chains.push(ChainCount { steps: chain, start_ext, count: 1, last: now, snoozed_at: 0 });
                if self.chains.len() > MAX_CHAINS {
                    let (i, _) = self
                        .chains
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, c)| (c.count, c.last))
                        .unwrap();
                    self.chains.remove(i);
                }
            }
        }
    }

    /// The suggestion for the chain that made `output`, if it has run often
    /// enough, isn't dismissed and isn't already a recipe (`is_saved`).
    pub fn suggestion_for(&self, output: &str, is_saved: impl Fn(&[String]) -> bool) -> Option<Suggestion> {
        let output_key = key(output);
        let made = self.recent.iter().rev().find(|p| key(&p.path) == output_key)?;
        let chain = self.chains.iter().find(|c| c.steps == made.steps)?;
        let due = chain.count >= THRESHOLD && chain.count >= chain.snoozed_at + THRESHOLD;
        (due && !self.dismissed.contains(&chain.steps) && !is_saved(&chain.steps)).then(|| Suggestion {
            steps: chain.steps.clone(),
            start_ext: made.start_ext.clone(),
            count: chain.count,
        })
    }

    /// "Not now": ask again after another few runs.
    pub fn snooze(&mut self, steps: &[String]) {
        if let Some(c) = self.chains.iter_mut().find(|c| c.steps == steps) {
            c.snoozed_at = c.count;
        }
    }

    /// "Don't suggest this": never offer this chain again.
    pub fn dismiss(&mut self, steps: &[String]) {
        if !self.dismissed.iter().any(|d| d == steps) {
            self.dismissed.push(steps.to_vec());
            if self.dismissed.len() > MAX_DISMISSED {
                self.dismissed.remove(0);
            }
        }
        self.chains.retain(|c| c.steps != steps);
    }
}

// ---------------------------------------------------------------- storage

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("suggestions.json"))
}

fn with_history<T>(app: &AppHandle, save: bool, f: impl FnOnce(&mut History) -> T) -> T {
    let mut guard = HISTORY.lock().unwrap();
    let history = guard.get_or_insert_with(|| {
        store_path(app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    });
    let out = f(history);
    if save {
        if let (Some(path), Ok(json)) = (store_path(app), serde_json::to_string(history)) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
    out
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Called after an action or recipe finishes. Does nothing when suggestions are off.
pub fn record(app: &AppHandle, input: &str, steps: &[String], output: &str) {
    if !crate::settings::current().suggest_recipes {
        return;
    }
    with_history(app, true, |h| h.record(input, steps, output, now()));
}

/// Forgets everything recorded (when suggestions are turned off).
pub fn clear(app: &AppHandle) {
    *HISTORY.lock().unwrap() = Some(History::default());
    if let Some(path) = store_path(app) {
        let _ = std::fs::remove_file(path);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionView {
    steps: Vec<String>,
    step_titles: Vec<String>,
    /// A file like the one the chain starts from, for the recipe editor.
    sample_path: String,
    count: u32,
}

/// What the drawer shows after a job that produced `path`, if anything.
#[tauri::command]
pub fn recipe_suggestion(app: AppHandle, path: String) -> Option<SuggestionView> {
    if !crate::settings::current().suggest_recipes {
        return None;
    }
    let saved = crate::recipes::all_steps(&app);
    let s = with_history(&app, false, |h| h.suggestion_for(&path, |steps| saved.iter().any(|r| r == steps)))?;
    let sample_path = format!("file.{}", s.start_ext);
    let (actions, _) = engine::plan_chain(Path::new(&sample_path), &s.steps).ok()?;
    Some(SuggestionView {
        step_titles: actions.iter().map(Action::title).collect(),
        steps: s.steps,
        sample_path,
        count: s.count,
    })
}

/// Hides a suggestion: for a while (`forever: false`) or for good.
#[tauri::command]
pub fn dismiss_suggestion(app: AppHandle, steps: Vec<String>, forever: bool) {
    with_history(&app, true, |h| if forever { h.dismiss(&steps) } else { h.snooze(&steps) });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// One "Resize 50% → Convert WebP" run on photo `n`.
    fn resize_then_webp(h: &mut History, n: u32, t: u64) -> String {
        let (input, small, webp) = (format!("C:\\p\\photo{n}.png"), format!("C:\\p\\photo{n}-50pct.png"), format!("C:\\p\\photo{n}-50pct.webp"));
        h.record(&input, &s(&["resize:50"]), &small, t);
        h.record(&small, &s(&["convert:webp"]), &webp, t + 5);
        webp
    }

    const NONE_SAVED: fn(&[String]) -> bool = |_| false;

    #[test]
    fn suggests_a_chain_after_three_runs() {
        let mut h = History::default();
        let first = resize_then_webp(&mut h, 1, 100);
        assert_eq!(h.suggestion_for(&first, NONE_SAVED), None);
        let second = resize_then_webp(&mut h, 2, 200);
        assert_eq!(h.suggestion_for(&second, NONE_SAVED), None);
        let third = resize_then_webp(&mut h, 3, 300);
        let got = h.suggestion_for(&third, NONE_SAVED).expect("suggested on the third run");
        assert_eq!(got.steps, s(&["resize:50", "convert:webp"]));
        assert_eq!(got.start_ext, "png");
        assert_eq!(got.count, 3);
        // Only for the chain's own output, not after an unrelated single step.
        h.record("C:\\p\\other.png", &s(&["convert:jpg"]), "C:\\p\\other.jpg", 400);
        assert_eq!(h.suggestion_for("C:\\p\\other.jpg", NONE_SAVED), None);
    }

    #[test]
    fn single_steps_and_unlinked_actions_are_not_chains() {
        let mut h = History::default();
        for n in 0..5 {
            let input = format!("C:\\p\\a{n}.png");
            // Two actions on the same original, not on each other's output.
            h.record(&input, &s(&["resize:50"]), &format!("C:\\p\\a{n}-50pct.png"), 10);
            h.record(&input, &s(&["convert:webp"]), &format!("C:\\p\\a{n}.webp"), 20);
        }
        assert!(h.chains.is_empty());
        assert_eq!(h.suggestion_for("C:\\p\\a4.webp", NONE_SAVED), None);
    }

    #[test]
    fn links_paths_case_insensitively_and_within_a_window() {
        let mut h = History::default();
        h.record("C:\\p\\x.png", &s(&["resize:50"]), "C:\\p\\X-50pct.png", 0);
        h.record("c:/p/x-50pct.png", &s(&["convert:webp"]), "c:/p/x-50pct.webp", 10);
        assert_eq!(h.chains[0].steps, s(&["resize:50", "convert:webp"]));
        // Picking the output up again much later starts over.
        h.record("C:\\p\\y.png", &s(&["resize:50"]), "C:\\p\\y-50pct.png", 0);
        h.record("C:\\p\\y-50pct.png", &s(&["convert:webp"]), "C:\\p\\y-50pct.webp", LINK_WINDOW_SECS + 1);
        assert_eq!(h.chains[0].count, 1);
    }

    #[test]
    fn longer_chains_and_recipe_runs_count_as_their_full_chain() {
        let mut h = History::default();
        for n in 0..3 {
            let a = format!("C:\\v\\clip{n}.mov");
            // A recipe (two steps) then one more action on its output.
            h.record(&a, &s(&["scale:720", "compress:28"]), &format!("C:\\v\\clip{n}-720p.mp4"), 1);
            h.record(&format!("C:\\v\\clip{n}-720p.mp4"), &s(&["audio:mp3"]), &format!("C:\\v\\clip{n}.mp3"), 2);
        }
        let got = h.suggestion_for("C:\\v\\clip2.mp3", NONE_SAVED).unwrap();
        assert_eq!(got.steps, s(&["scale:720", "compress:28", "audio:mp3"]));
        assert_eq!(got.start_ext, "mov");
    }

    #[test]
    fn saved_dismissed_and_snoozed_chains_stay_quiet() {
        let mut h = History::default();
        let mut last = String::new();
        for n in 0..3 {
            last = resize_then_webp(&mut h, n, n as u64 * 100);
        }
        let steps = s(&["resize:50", "convert:webp"]);
        assert!(h.suggestion_for(&last, |c| c == steps.as_slice()).is_none(), "already a recipe");

        h.snooze(&steps);
        assert!(h.suggestion_for(&last, NONE_SAVED).is_none());
        for n in 3..5 {
            last = resize_then_webp(&mut h, n, 1000);
        }
        assert!(h.suggestion_for(&last, NONE_SAVED).is_none(), "snoozed for three more runs");
        last = resize_then_webp(&mut h, 5, 1000);
        assert!(h.suggestion_for(&last, NONE_SAVED).is_some());

        h.dismiss(&steps);
        for n in 6..12 {
            last = resize_then_webp(&mut h, n, 2000);
        }
        assert!(h.suggestion_for(&last, NONE_SAVED).is_none(), "dismissed for good");
    }

    #[test]
    fn history_stays_capped() {
        let mut h = History::default();
        for n in 0..(MAX_CHAINS as u32 + 40) {
            // A distinct two-step chain per run.
            let a = format!("C:\\p\\{n}.png");
            let b = format!("C:\\p\\{n}-r.png");
            h.record(&a, &s(&["resize:50"]), &b, n as u64);
            h.record(&b, &[format!("fitkb:{n}")], &format!("C:\\p\\{n}.jpg"), n as u64);
        }
        for n in 0..(MAX_DISMISSED + 10) {
            h.dismiss(&[format!("x:{n}"), "y:1".into()]);
        }
        assert!(h.recent.len() <= MAX_RECENT);
        assert!(h.chains.len() <= MAX_CHAINS);
        assert!(h.dismissed.len() <= MAX_DISMISSED);
        // Runaway chains don't grow without bound either.
        let mut path = "C:\\p\\start.png".to_string();
        for n in 0..(MAX_STEPS * 3) {
            let next = format!("C:\\p\\step{n}.png");
            h.record(&path, &s(&["resize:75"]), &next, 5000);
            path = next;
        }
        assert!(h.recent.iter().all(|p| p.steps.len() <= MAX_STEPS));
    }
}
