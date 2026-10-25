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
}

const BUILTIN_PREFIX: &str = "builtin:";

fn builtins() -> Vec<Recipe> {
    let r = |id: &str, name: &str, steps: &[&str]| Recipe {
        id: format!("{BUILTIN_PREFIX}{id}"),
        name: name.into(),
        steps: steps.iter().map(|s| s.to_string()).collect(),
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

fn load_custom(app: &AppHandle) -> Vec<Recipe> {
    store_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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

fn all(app: &AppHandle) -> Vec<Recipe> {
    let mut v = builtins();
    v.extend(load_custom(app));
    v
}

/// Steps of every recipe, built-in and custom (to skip suggesting a saved one).
pub fn all_steps(app: &AppHandle) -> Vec<Vec<String>> {
    all(app).into_iter().map(|r| r.steps).collect()
}

fn view(recipe: Recipe, actions: Vec<Action>) -> RecipeView {
    RecipeView {
        builtin: recipe.id.starts_with(BUILTIN_PREFIX),
        step_titles: actions.iter().map(Action::title).collect(),
        id: recipe.id,
        name: recipe.name,
        steps: recipe.steps,
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

/// Creates (no `id`) or updates a custom recipe. Returns its id.
#[tauri::command]
pub fn save_recipe(
    app: AppHandle,
    id: Option<String>,
    name: String,
    steps: Vec<String>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the recipe a name".into());
    }
    if steps.is_empty() {
        return Err("Add at least one step".into());
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
            id
        }
        None => {
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            let id = format!("custom:{millis}");
            custom.push(Recipe { id: id.clone(), name: name.into(), steps });
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
        let report = |f: f32| crate::emit_progress(&app, &label, &path, f);
        let out = engine::run_chain(Path::new(&path), &recipe.steps, &report)
            .map(|p| p.to_string_lossy().into_owned())?;
        crate::suggest::record(&app, &path, &recipe.steps, &out);
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
