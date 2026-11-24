#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use filecraft_core::{
    Capability, Control, Inspection, Phase, Progress, Recipe, capabilities, inspect, run,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Manager, State, async_runtime};
use tokio::sync::Semaphore;

type CommandResult<T> = Result<T, String>;

#[derive(Clone)]
struct Jobs {
    active: Arc<Mutex<HashMap<String, Active>>>,
    history: Arc<Mutex<Connection>>,
    worker: Arc<Semaphore>,
    inspector: Arc<Semaphore>,
    _instance_lock: Arc<std::fs::File>,
}

struct Active {
    job: Job,
    control: Arc<Control>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    created_ms: i64,
    state: String,
    count: usize,
    progress: Option<Value>,
    report: Option<Value>,
    error: Option<String>,
}

#[derive(Serialize)]
struct Inspected {
    path: PathBuf,
    file: Option<Inspection>,
    error: Option<String>,
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Jobs {
    fn open(path: PathBuf) -> CommandResult<Self> {
        std::fs::create_dir_all(&path).map_err(message)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("instance.lock"))
            .map_err(message)?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|error| format!("FileCraft could not lock its local job store: {error}"))?;
        let history = Connection::open(path.join("history.sqlite3")).map_err(message)?;
        history.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY, created_ms INTEGER NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL
            );
            UPDATE jobs SET state = 'failed',
                payload = json_set(payload, '$.state', 'failed', '$.error',
                    'FileCraft closed during this job. Check the output folder before retrying.')
            WHERE state NOT IN ('completed', 'partial', 'failed', 'cancelled');"
        ).map_err(message)?;
        Ok(Self {
            active: Arc::default(),
            history: Arc::new(Mutex::new(history)),
            worker: Arc::new(Semaphore::new(1)),
            inspector: Arc::new(Semaphore::new(1)),
            _instance_lock: Arc::new(lock),
        })
    }

    fn save(&self, job: &Job) -> CommandResult<()> {
        self.history
            .lock()
            .map_err(message)?
            .execute(
                "INSERT INTO jobs (id, created_ms, state, payload) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(id) DO UPDATE SET state=excluded.state, payload=excluded.payload",
                params![
                    job.id,
                    job.created_ms,
                    job.state,
                    serde_json::to_string(job).map_err(message)?
                ],
            )
            .map_err(message)?;
        Ok(())
    }

    fn get(&self, id: &str) -> CommandResult<Job> {
        if let Some(entry) = self.active.lock().map_err(message)?.get(id) {
            return Ok(entry.job.clone());
        }
        let payload: String = self
            .history
            .lock()
            .map_err(message)?
            .query_row("SELECT payload FROM jobs WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .map_err(message)?;
        serde_json::from_str(&payload).map_err(message)
    }
}

#[tauri::command]
async fn engine_capabilities() -> CommandResult<[Capability; 3]> {
    async_runtime::spawn_blocking(capabilities)
        .await
        .map_err(message)
}

#[tauri::command]
async fn inspect_files(
    paths: Vec<PathBuf>,
    jobs: State<'_, Jobs>,
) -> CommandResult<Vec<Inspected>> {
    let _permit = jobs.inspector.acquire().await.map_err(message)?;
    async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                let (file, error) = match inspect(&path) {
                    Ok(file) => (Some(file), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                Inspected { path, file, error }
            })
            .collect()
    })
    .await
    .map_err(message)
}

#[tauri::command]
async fn import_recipe(path: PathBuf) -> CommandResult<Recipe> {
    async_runtime::spawn_blocking(move || Recipe::read(&path).map_err(message))
        .await
        .map_err(message)?
}

#[tauri::command]
fn export_recipe(path: PathBuf, recipe: Recipe) -> CommandResult<()> {
    recipe.validate().map_err(message)?;
    export_json(path, &recipe)
}

#[tauri::command]
fn export_report(path: PathBuf, id: String, jobs: State<'_, Jobs>) -> CommandResult<()> {
    let job = jobs.get(&id)?;
    let report = job
        .report
        .ok_or_else(|| "This job has no execution report".to_string())?;
    export_json(path, &report)
}

fn export_json(path: PathBuf, value: &impl Serialize) -> CommandResult<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(message)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(message)?;
    file.write_all(&bytes).map_err(message)
}

#[tauri::command]
fn start_job(
    paths: Vec<PathBuf>,
    destination: PathBuf,
    recipe: Recipe,
    jobs: State<'_, Jobs>,
) -> CommandResult<Job> {
    recipe.validate().map_err(message)?;
    if paths.is_empty() {
        return Err("Add at least one file".into());
    }
    let jobs = jobs.inner().clone();
    let job = Job {
        id: uuid::Uuid::new_v4().to_string(),
        created_ms: i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(message)?
                .as_millis(),
        )
        .map_err(message)?,
        state: "pending".into(),
        count: paths.len(),
        progress: None,
        report: None,
        error: None,
    };
    let id = job.id.clone();
    let control = Arc::new(Control::default());
    jobs.save(&job)?;
    jobs.active.lock().map_err(message)?.insert(
        id.clone(),
        Active {
            job: job.clone(),
            control: control.clone(),
        },
    );
    async_runtime::spawn(async move {
        let result = async {
            let _permit = jobs.worker.acquire().await.map_err(message)?;
            let active = jobs.active.clone();
            let worker_id = id.clone();
            async_runtime::spawn_blocking(move || {
                run(
                    &paths,
                    &destination,
                    &recipe,
                    &control,
                    |progress: Progress| {
                        if let Ok(mut active) = active.lock()
                            && let Some(entry) = active.get_mut(&worker_id)
                        {
                            entry.job.state = match progress.phase {
                                Phase::Inspecting => "inspecting",
                                Phase::Planning => "planning",
                                Phase::Running => "running",
                                Phase::Verifying => "verifying",
                            }
                            .into();
                            entry.job.progress = serde_json::to_value(progress).ok();
                        }
                    },
                )
            })
            .await
            .map_err(message)
        }
        .await;
        if let Ok(mut finished) = jobs.get(&id) {
            match result.and_then(|report| serde_json::to_value(report).map_err(message)) {
                Ok(report) => {
                    finished.state = report["status"].as_str().unwrap_or("failed").into();
                    finished.error = report["error"].as_str().map(str::to_owned);
                    finished.report = Some(report);
                }
                Err(error) => {
                    finished.state = "failed".into();
                    finished.error = Some(error);
                }
            }
            let saved = jobs.save(&finished);
            if let Ok(mut active) = jobs.active.lock() {
                match saved {
                    Ok(()) => {
                        active.remove(&id);
                    }
                    Err(error) => {
                        let previous = finished.error.take();
                        finished.error = Some(previous.map_or_else(
                            || format!("History could not be saved: {error}"),
                            |previous| format!("{previous}. History could not be saved: {error}"),
                        ));
                        if let Some(entry) = active.get_mut(&id) {
                            entry.job = finished;
                        }
                    }
                }
            }
        }
    });
    Ok(job)
}

#[tauri::command]
fn job_status(id: String, jobs: State<'_, Jobs>) -> CommandResult<Job> {
    jobs.get(&id)
}

#[tauri::command]
fn cancel_job(id: String, jobs: State<'_, Jobs>) -> CommandResult<()> {
    if let Some(entry) = jobs.active.lock().map_err(message)?.get(&id) {
        entry.control.cancel();
    }
    Ok(())
}

#[tauri::command]
fn job_history(jobs: State<'_, Jobs>) -> CommandResult<Vec<Job>> {
    let history = jobs.history.lock().map_err(message)?;
    let mut query = history
        .prepare("SELECT payload FROM jobs ORDER BY created_ms DESC LIMIT 100")
        .map_err(message)?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(message)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(serde_json::from_str::<Job>(&row.map_err(message)?).map_err(message)?);
    }
    drop(query);
    drop(history);
    let active = jobs.active.lock().map_err(message)?;
    for job in &mut result {
        if let Some(entry) = active.get(&job.id) {
            *job = entry.job.clone();
        }
    }
    Ok(result)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Jobs::open(app.path().app_local_data_dir()?)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_capabilities,
            inspect_files,
            import_recipe,
            export_recipe,
            export_report,
            start_job,
            job_status,
            cancel_job,
            job_history
        ])
        .run(tauri::generate_context!())
        .expect("FileCraft desktop could not start");
}
