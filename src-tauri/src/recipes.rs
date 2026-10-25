//! Recipes: named chains of actions, e.g. "Resize 50%" then "Convert WebP".
//! Built-in recipes ship with the app; custom ones are stored as JSON in the
//! app config directory.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::engine::{self, Action};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    /// Action ids, run in order.
    pub steps: Vec<String>,
    /// Saves here instead of the "Save to" folder in Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
}

/// What the drawer gets: a recipe that applies to the dropped file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeView {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub step_titles: Vec<String>,
    pub builtin: bool,
    pub output_dir: Option<String>,
}

const BUILTIN_PREFIX: &str = "builtin:";

fn builtins() -> Vec<Recipe> {
    let r = |id: &str, name: &str, steps: &[&str]| Recipe {
        id: format!("{BUILTIN_PREFIX}{id}"),
        name: name.into(),
        steps: steps.iter().map(|s| s.to_string()).collect(),
        output_dir: None,
    };
    vec![
        r("web-image", "Web-ready image", &["resize:50", "convert:webp"]),
        r("email-photo", "Email-friendly photo", &["resize:50", "compress:75"]),
        r("share-clip", "Share-size clip", &["scale:720", "compress:28"]),
        r("clip-gif", "Clip to GIF", &["scale:480", "convert:gif"]),
        r("podcast", "Podcast MP3", &["convert:mp3"]),
        r("clip-10mb", "Clip under 10 MB", &["fitmb:10"]),
    ]
}

fn store_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("recipes.json"))
}

fn load_custom_from(path: &Path) -> Vec<Recipe> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn load_custom(app: &AppHandle) -> Vec<Recipe> {
    store_path(app).map(|p| load_custom_from(&p)).unwrap_or_default()
}

/// Built-in and saved recipes, read from `config_dir` (for the CLI, which
/// runs without the app).
pub fn all_in(config_dir: &Path) -> Vec<Recipe> {
    let mut v = builtins();
    v.extend(load_custom_from(&config_dir.join("recipes.json")));
    v
}

/// The recipe `query` names: its id, its id without `builtin:`, or its name
/// (any case).
pub fn lookup(recipes: Vec<Recipe>, query: &str) -> Option<Recipe> {
    let q = query.trim();
    let short = |r: &Recipe| r.id.strip_prefix(BUILTIN_PREFIX).map(str::to_string);
    let by_id = recipes.iter().position(|r| r.id == q || short(r).as_deref() == Some(q));
    let i = by_id.or_else(|| recipes.iter().position(|r| r.name.eq_ignore_ascii_case(q)))?;
    recipes.into_iter().nth(i)
}

pub fn is_builtin(id: &str) -> bool {
    id.starts_with(BUILTIN_PREFIX)
}

fn save_custom(app: &AppHandle, recipes: &[Recipe]) -> Result<(), String> {
    let path = store_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(recipes).map_err(|e| e.to_string())?;
    // Write then rename so a crash can't leave a half-written file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn all(app: &AppHandle) -> Vec<Recipe> {
    let mut v = builtins();
    v.extend(load_custom(app));
    v
}

fn view(recipe: Recipe, actions: Vec<Action>) -> RecipeView {
    RecipeView {
        builtin: is_builtin(&recipe.id),
        step_titles: actions.iter().map(Action::title).collect(),
        id: recipe.id,
        name: recipe.name,
        steps: recipe.steps,
        output_dir: recipe.output_dir,
    }
}

/// Recipes whose whole chain can run on `path`.
#[tauri::command]
pub fn recipes_for(app: AppHandle, path: String) -> Vec<RecipeView> {
    all(&app)
        .into_iter()
        .filter_map(|r| {
            let (actions, _) = engine::plan_chain(Path::new(&path), &r.steps).ok()?;
            Some(view(r, actions))
        })
        .collect()
}

/// Actions the editor can offer after `steps`, for a file like `path`.
#[tauri::command]
pub fn next_actions(path: String, steps: Vec<String>) -> Result<Vec<Action>, String> {
    engine::next_actions(Path::new(&path), &steps)
}

/// "Custom…" entries the editor can offer after `steps`.
#[tauri::command]
pub fn next_custom_forms(path: String, steps: Vec<String>) -> Result<Vec<engine::params::CustomForm>, String> {
    engine::next_custom_forms(Path::new(&path), &steps)
}

/// Checks that `step` can follow `steps` and returns it with its title.
#[tauri::command]
pub fn check_step(path: String, steps: Vec<String>, step: String) -> Result<Action, String> {
    let mut all = steps;
    all.push(step);
    let (mut actions, _) = engine::plan_chain(Path::new(&path), &all)?;
    Ok(actions.pop().expect("at least one step"))
}

/// Folder picker for a recipe's own "Save to", parented to the drawer.
#[tauri::command]
pub async fn pick_recipe_folder(app: AppHandle, window: tauri::WebviewWindow) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("Save this recipe's files to…")
        .set_parent(&window)
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Creates (no `id`) or updates a custom recipe. Returns its id.
#[tauri::command]
pub fn save_recipe(
    app: AppHandle,
    id: Option<String>,
    name: String,
    steps: Vec<String>,
    output_dir: Option<String>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the recipe a name".into());
    }
    if steps.is_empty() {
        return Err("Add at least one step".into());
    }
    let output_dir = output_dir.filter(|d| !d.trim().is_empty());
    if let Some(dir) = &output_dir {
        if !Path::new(dir).is_dir() {
            return Err(format!("The folder {dir} doesn't exist"));
        }
    }
    let mut custom = load_custom(&app);
    let id = match id {
        Some(id) if id.starts_with(BUILTIN_PREFIX) => {
            return Err("Built-in recipes can't be changed".into())
        }
        Some(id) => {
            let r = custom
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("That recipe no longer exists")?;
            r.name = name.into();
            r.steps = steps;
            r.output_dir = output_dir;
            id
        }
        None => {
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            let id = format!("custom:{millis}");
            custom.push(Recipe { id: id.clone(), name: name.into(), steps, output_dir });
            id
        }
    };
    save_custom(&app, &custom)?;
    Ok(id)
}

#[tauri::command]
pub fn delete_recipe(app: AppHandle, id: String) -> Result<(), String> {
    let mut custom = load_custom(&app);
    let before = custom.len();
    custom.retain(|r| r.id != id);
    if custom.len() == before {
        return Err("Only custom recipes can be deleted".into());
    }
    save_custom(&app, &custom)
}

#[tauri::command]
pub async fn run_recipe(
    app: AppHandle,
    window: tauri::WebviewWindow,
    id: String,
    path: String,
    job_id: Option<String>,
) -> Result<String, String> {
    let label = window.label().to_string();
    let recipe = all(&app)
        .into_iter()
        .find(|r| r.id == id)
        .ok_or("That recipe no longer exists")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _job = crate::engine::jobs::begin(job_id.as_deref());
        let _dir = engine::override_output_dir(recipe.output_dir.map(PathBuf::from));
        let report = |f: f32| crate::emit_progress(&app, &label, &path, f);
        let input = Path::new(&path);
        let out = engine::run_chain(input, &recipe.steps, &report)?;
        engine::naming::finish(input, out, true).map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_recipe_files_still_load() {
        let old = r#"[{"id":"custom:1","name":"Thumb","steps":["resize:25","convert:jpg"]}]"#;
        let r: Vec<Recipe> = serde_json::from_str(old).unwrap();
        assert_eq!(r[0].output_dir, None);
        // Custom values chain like the fixed ones.
        let steps = ["resize:w1280".to_string(), "compress:q62".to_string()];
        let (actions, ext) = engine::plan_chain(Path::new("a.png"), &steps).unwrap();
        assert_eq!(ext, "jpg");
        assert_eq!(actions[0].title(), "Resize 1280 px wide");
        let err = engine::plan_chain(Path::new("a.png"), &["resize:w0".to_string()]).unwrap_err();
        assert_eq!(err, "Step 1: Width must be a whole number from 1 to 20000 px");
    }

    #[test]
    fn builtins_are_valid_chains() {
        let samples = ["a.png", "a.jpg", "a.mov", "a.mp4", "a.wav"];
        for r in builtins() {
            assert!(
                samples.iter().any(|s| engine::plan_chain(Path::new(s), &r.steps).is_ok()),
                "built-in recipe {} applies to no sample file",
                r.name
            );
        }
    }

    #[test]
    fn looks_up_by_id_short_id_or_name() {
        let found = |q: &str| lookup(builtins(), q).map(|r| r.id);
        assert_eq!(found("builtin:podcast").as_deref(), Some("builtin:podcast"));
        assert_eq!(found("web-image").as_deref(), Some("builtin:web-image"));
        assert_eq!(found("clip to gif").as_deref(), Some("builtin:clip-gif"));
        assert_eq!(found("nope"), None);
    }
}
