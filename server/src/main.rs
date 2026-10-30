//! OPM Studio server: REST + live WebSocket for the editor, MCP for ChatGPT, JSON files on disk.
use axum::{
    extract::{ws::Message, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use opm_core::{Id, Model, Op};
use std::{
    collections::HashMap,
    hash::{BuildHasher, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

mod mcp;

pub struct App {
    // ponytail: one global lock; per-model locks if real concurrency shows up.
    models: Mutex<HashMap<String, Model>>,
    /// (model id, model JSON) after every change.
    tx: broadcast::Sender<(String, Arc<str>)>,
    dir: PathBuf,
    token: String,
}

pub fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

impl App {
    fn load(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).expect("create data dir");
        let mut models = HashMap::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                match std::fs::read_to_string(&path).map(|s| serde_json::from_str::<Model>(&s)) {
                    Ok(Ok(mut m)) => {
                        m.normalize();
                        models.insert(path.file_stem().unwrap().to_string_lossy().into_owned(), m);
                    }
                    other => eprintln!("skipping {}: {other:?}", path.display()),
                }
            }
        }
        let token = mcp_token(&dir);
        App { models: Mutex::new(models), tx: broadcast::channel(64).0, dir, token }
    }

    pub fn list(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self.models.lock().unwrap().iter().map(|(k, m)| (k.clone(), m.name.clone())).collect();
        v.sort();
        v
    }

    pub fn get(&self, id: &str) -> Option<Model> {
        self.models.lock().unwrap().get(id).cloned()
    }

    /// Creates the model if missing; returns its id either way.
    pub fn create(&self, name: &str) -> Result<String, String> {
        let id = slug(name);
        if id.is_empty() {
            return Err("model name needs letters or digits".into());
        }
        if self.get(&id).is_none() {
            self.save(&id, Model::new(name.trim()))?;
        }
        Ok(id)
    }

    /// Runs `f` on a copy of the model; only a successful run is kept, saved and broadcast.
    pub fn edit<T>(&self, id: &str, f: impl FnOnce(&mut Model) -> Result<T, String>) -> Result<T, String> {
        let mut m = self.get(id).ok_or_else(|| format!("no model '{id}'"))?;
        let out = f(&mut m)?;
        self.save(id, m)?;
        Ok(out)
    }

    /// Moves the model's file to `data/.trash/` (recoverable by moving it back and restarting).
    pub fn trash(&self, id: &str) -> Result<(), String> {
        // Only known ids (slugs) reach the file system.
        self.models.lock().unwrap().remove(id).ok_or_else(|| format!("no model '{id}'"))?;
        let trash = self.dir.join(".trash");
        std::fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        std::fs::rename(self.dir.join(format!("{id}.json")), trash.join(format!("{id}-{stamp}.json"))).map_err(|e| e.to_string())
    }

    /// Adds `src` (an exported model, old or current format) as a new model, renamed "X (2)" if the name is taken.
    pub fn import(&self, mut src: Model) -> Result<String, String> {
        src.normalize();
        src.validate()?;
        let base = match src.name.trim() {
            "" => "Imported",
            n => n,
        };
        let name = (1..)
            .map(|i| if i == 1 { base.to_string() } else { format!("{base} ({i})") })
            .find(|n| !slug(n).is_empty() && self.get(&slug(n)).is_none())
            .ok_or("model name needs letters or digits")?;
        src.name = name.clone();
        let id = slug(&name);
        self.save(&id, src)?;
        Ok(id)
    }

    fn save(&self, id: &str, m: Model) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
        std::fs::write(self.dir.join(format!("{id}.json")), &json).map_err(|e| e.to_string())?;
        self.models.lock().unwrap().insert(id.to_string(), m);
        let _ = self.tx.send((id.to_string(), json.into()));
        Ok(())
    }
}

fn err(e: String) -> Response {
    (StatusCode::BAD_REQUEST, e).into_response()
}

async fn list_models(State(app): State<Arc<App>>) -> Json<Vec<(String, String)>> {
    Json(app.list())
}

#[derive(serde::Deserialize)]
struct NewModel {
    name: String,
}

async fn create_model(State(app): State<Arc<App>>, Json(b): Json<NewModel>) -> Response {
    app.create(&b.name).map(|id| Json(id).into_response()).unwrap_or_else(err)
}

async fn get_model(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    app.get(&id).map(|m| Json(m).into_response()).unwrap_or(StatusCode::NOT_FOUND.into_response())
}

/// Whole-model replace, used by the editor's undo/redo.
async fn replace_model(State(app): State<Arc<App>>, Path(id): Path<String>, Json(mut new): Json<Model>) -> Response {
    new.normalize();
    if let Err(e) = new.validate() {
        return err(e);
    }
    app.edit(&id, |m| Ok(*m = new)).map(|_| StatusCode::NO_CONTENT.into_response()).unwrap_or_else(err)
}

async fn rename_model(State(app): State<Arc<App>>, Path(id): Path<String>, Json(b): Json<NewModel>) -> Response {
    let name = b.name.trim().to_string();
    if name.is_empty() {
        return err("name can't be empty".into());
    }
    app.edit(&id, |m| Ok(m.name = name)).map(|_| StatusCode::NO_CONTENT.into_response()).unwrap_or_else(err)
}

async fn delete_model(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    app.trash(&id).map(|_| StatusCode::NO_CONTENT.into_response()).unwrap_or_else(err)
}

async fn import_model(State(app): State<Arc<App>>, Json(m): Json<Model>) -> Response {
    app.import(m).map(|id| Json(id).into_response()).unwrap_or_else(err)
}

async fn mcp_info(State(app): State<Arc<App>>) -> Json<String> {
    Json(app.token.clone())
}

/// The tunnel exposes the whole port; only MCP (behind its secret path) may be reached through it.
async fn local_only(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let host = req.headers().get("host").and_then(|h| h.to_str().ok()).unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let tunneled = ["x-forwarded-for", "tailscale-funnel-request", "cf-connecting-ip"].iter().any(|h| req.headers().contains_key(*h));
    if tunneled || !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

/// Applies all ops or none.
async fn apply_ops(State(app): State<Arc<App>>, Path(id): Path<String>, Json(ops): Json<Vec<Op>>) -> Response {
    app.edit(&id, |m| ops.into_iter().map(|op| m.apply(op)).collect::<Result<Vec<Id>, _>>())
        .map(|ids| Json(ids).into_response())
        .unwrap_or_else(err)
}

async fn ws(State(app): State<Arc<App>>, Path(id): Path<String>, up: WebSocketUpgrade) -> Response {
    up.on_upgrade(move |mut socket| async move {
        let mut rx = app.tx.subscribe();
        if let Some(m) = app.get(&id) {
            let json = serde_json::to_string(&m).unwrap();
            if socket.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok((mid, json)) if mid == id => {
                        if socket.send(Message::Text(json.to_string().into())).await.is_err() { break; }
                    }
                    Ok(_) => {}
                    // Missed updates: resend the current state.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let Some(m) = app.get(&id) else { break };
                        let json = serde_json::to_string(&m).unwrap();
                        if socket.send(Message::Text(json.into())).await.is_err() { break; }
                    }
                    Err(_) => break,
                },
                incoming = socket.recv() => if !matches!(incoming, Some(Ok(_))) { break },
            }
        }
    })
}

/// Secret MCP path segment, generated once and kept in the data dir.
fn mcp_token(dir: &std::path::Path) -> String {
    if let Ok(t) = std::env::var("OPM_MCP_TOKEN") {
        return t;
    }
    let path = dir.join(".mcp_token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        return t.trim().to_string();
    }
    // RandomState is seeded from OS randomness per instance.
    let r = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    let t = format!("{:016x}{:016x}", r(), r());
    std::fs::write(&path, &t).expect("write token");
    t
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let app = Arc::new(App::load(root.join("data")));
    let token = app.token.clone();
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());

    let local = Router::new()
        .route("/api/models", get(list_models).post(create_model))
        .route("/api/models/{id}", get(get_model).put(replace_model).delete(delete_model))
        .route("/api/models/{id}/rename", post(rename_model))
        .route("/api/import", post(import_model))
        .route("/api/models/{id}/ops", post(apply_ops))
        .route("/api/mcp", get(mcp_info))
        .route("/ws/{id}", get(ws))
        .fallback_service(
            tower::ServiceBuilder::new()
                // Always revalidate so a fresh `trunk build` shows up on reload.
                .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-cache"),
                ))
                .service(tower_http::services::ServeDir::new(root.join("web/dist"))),
        )
        .layer(axum::middleware::from_fn(local_only))
        .with_state(app.clone());
    let router = Router::new().nest_service(&format!("/mcp/{token}"), mcp::service(app)).merge(local);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    println!("OPM Studio:  http://localhost:{port}");
    println!("MCP (local): http://localhost:{port}/mcp/{token}");
    println!("ChatGPT:     tailscale funnel --bg {port}  ->  https://<pc>.<tailnet>.ts.net/mcp/{token}");
    axum::serve(listener, router).await.unwrap();
}
