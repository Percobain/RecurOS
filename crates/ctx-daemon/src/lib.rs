//! `ctx daemon`: the local HTTP API the browser extension talks to
//! (docs/protocol.md §3).
//!
//! Binds loopback only and requires a bearer token on every request except
//! `/v1/health`. Requests carrying a web-page `Origin` are refused outright:
//! the extension's service worker is the only intended browser caller, and
//! this stops any website from even probing the API.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ctx_app::{App, ClaimInput, PackOpts};
use ctx_core::{Filter, Store};
use ctx_pack::Projection;
use serde::Deserialize;
use serde_json::json;

pub const DEFAULT_ADDR: &str = "127.0.0.1:7777";

/// How often an idle daemon pulls other machines' claims (spec §6.5).
pub const IDLE_SYNC: Duration = Duration::from_secs(300);

/// `~/.ctx/token`, or `$CTX_TOKEN_FILE`.
pub fn token_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CTX_TOKEN_FILE").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".ctx").join("token"))
}

/// Read the daemon token, creating a random one (0600 on Unix) if missing.
pub fn ensure_token() -> Result<String> {
    let path = token_path()?;
    if let Ok(t) = std::fs::read_to_string(&path)
        && !t.trim().is_empty()
    {
        return Ok(t.trim().to_owned());
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no OS randomness: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_private(&path, &token)?;
    Ok(token)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    // Files under the user profile on Windows are private to the user by
    // default ACLs.
    std::fs::write(path, contents)?;
    Ok(())
}

#[derive(Clone)]
struct AppState {
    app: Arc<Mutex<App>>,
    token: Arc<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: impl std::fmt::Display) -> ApiError {
    (status, Json(json!({"error": msg.to_string()})))
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok())
        && (origin.starts_with("http://") || origin.starts_with("https://"))
    {
        return Err(err(StatusCode::FORBIDDEN, "web origins are not allowed"));
    }
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !constant_time_eq(given.as_bytes(), state.token.as_bytes()) {
        return Err(err(
            StatusCode::UNAUTHORIZED,
            "missing or wrong token: paste the output of `ctx daemon token` into the extension options",
        ));
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn with_app<T>(state: &AppState, f: impl FnOnce(&mut App) -> Result<T>) -> Result<T, ApiError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "state poisoned"))?;
    app.catch_up()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
    f(&mut app).map_err(|e| err(StatusCode::BAD_REQUEST, format!("{e:#}")))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "version": ctx_app::VERSION}))
}

async fn branches(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    authorize(&s, &headers)?;
    with_app(&s, |app| {
        let mut list: Vec<serde_json::Value> = Vec::new();
        let counts = app.store.branches()?;
        let mut names: Vec<String> = app.branches.all().into_iter().map(String::from).collect();
        for c in &counts {
            if !names.contains(&c.branch) {
                names.push(c.branch.clone());
            }
        }
        names.sort();
        for n in names {
            let claims = counts
                .iter()
                .find(|c| c.branch == n)
                .map(|c| c.active)
                .unwrap_or(0);
            list.push(json!({"name": n, "claims": claims}));
        }
        Ok(Json(json!({
            "active": app.default_branch()?.to_string(),
            "branches": list
        })))
    })
}

#[derive(Deserialize)]
struct PackQuery {
    branch: Option<String>,
    task: Option<String>,
    budget: Option<u32>,
    #[serde(rename = "for")]
    projection: Option<String>,
}

async fn pack(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PackQuery>,
) -> Result<Response, ApiError> {
    authorize(&s, &headers)?;
    with_app(&s, |app| {
        let markdown = if q.projection.as_deref() == Some("index") {
            app.index()?
        } else {
            let projection: Projection = q
                .projection
                .as_deref()
                .unwrap_or("dossier")
                .parse()
                .map_err(anyhow::Error::msg)?;
            app.pack(&PackOpts {
                branch: q.branch.clone().filter(|b| !b.is_empty()),
                task: q.task.clone(),
                budget: q.budget.map(|b| b.clamp(100, 200_000)),
                projection: Some(projection),
            })?
            .markdown
        };
        Ok((
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            markdown,
        )
            .into_response())
    })
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    branch: Option<String>,
}

async fn search(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    authorize(&s, &headers)?;
    with_app(&s, |app| {
        let branch = q
            .branch
            .as_deref()
            .map(|b| app.resolve_branch(Some(b)))
            .transpose()?;
        let claims = app.store.search(
            &q.q,
            &Filter {
                branch,
                limit: Some(50),
                ..Default::default()
            },
        )?;
        Ok(Json(json!({"claims": claims})))
    })
}

#[derive(Deserialize)]
struct ClaimsBody {
    branch: Option<String>,
    src: Option<String>,
    claims: Vec<ClaimInput>,
}

async fn claims(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClaimsBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    authorize(&s, &headers)?;
    if body.claims.len() > 200 {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "at most 200 claims per request",
        ));
    }
    with_app(&s, |app| {
        let branch = app.resolve_branch(body.branch.as_deref())?;
        let src = body
            .src
            .as_deref()
            .map(|s| {
                s.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c))
                    .take(40)
                    .collect()
            })
            .filter(|s: &String| !s.is_empty())
            .unwrap_or_else(|| "browser".to_owned());
        let report = app.save_inputs(&body.claims, &branch, &src);
        Ok(Json(serde_json::to_value(report)?))
    })
}

/// The router, separate from `serve` so tests can drive it without a socket.
pub fn router(app: App, token: String) -> Router {
    let state = AppState {
        app: Arc::new(Mutex::new(app)),
        token: Arc::new(token),
    };
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/branches", get(branches))
        .route("/v1/pack", get(pack))
        .route("/v1/search", get(search))
        .route("/v1/claims", post(claims))
        .with_state(state)
}

/// Run the daemon until interrupted. Pulls other machines' claims every
/// [`IDLE_SYNC`] when a remote is configured.
pub fn serve(app: App, addr: &str) -> Result<()> {
    let token = ensure_token()?;
    let router = router(app, token);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding {addr} (is another `ctx daemon` running?)"))?;
        eprintln!("ctx daemon listening on http://{addr} (loopback only). Ctrl-C to stop.");
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok(())
    })
}

/// Background sync loop, run on its own thread with its own `App`: sync does
/// network I/O and must not hold the request lock.
pub fn spawn_idle_sync(open: impl Fn() -> Result<App> + Send + 'static) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(IDLE_SYNC);
            match open().and_then(|mut app| app.sync()) {
                Ok(r) if r.pulled || r.pushed => eprintln!("ctx daemon: synced"),
                Ok(_) => {}
                Err(e) => eprintln!("ctx daemon: sync failed (will retry): {e:#}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use ctx_git::CtxHome;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn setup() -> (tempfile::TempDir, Router) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(CtxHome::at(dir.path().join("ctx")), None).unwrap();
        (dir, router(app, "secret".into()))
    }

    async fn send(r: &Router, req: Request<Body>) -> (StatusCode, String) {
        let resp = r.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn authed(method: &str, uri: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer secret")
            .header("content-type", "application/json")
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn auth_origin_and_round_trip() {
        let (_d, r) = setup();
        let (st, _) = send(&r, Request::get("/v1/health").body(Body::empty()).unwrap()).await;
        assert_eq!(st, StatusCode::OK);

        let (st, _) = send(
            &r,
            Request::get("/v1/branches").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        let mut evil = authed("GET", "/v1/branches", Body::empty());
        evil.headers_mut()
            .insert("origin", "https://evil.example".parse().unwrap());
        assert_eq!(send(&r, evil).await.0, StatusCode::FORBIDDEN);

        let body = r#"{"src":"claude.ai","claims":[
            {"kind":"decision","text":"Use a peg","why":"fast"},
            {"kind":"decision","text":"Use a peg","why":"fast"},
            {"kind":"opinion","text":"bad kind"}]}"#;
        let (st, out) = send(&r, authed("POST", "/v1/claims", Body::from(body))).await;
        assert_eq!(st, StatusCode::OK, "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["saved"].as_array().unwrap().len(), 1);
        assert_eq!(v["duplicates"].as_array().unwrap().len(), 1);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);

        let (st, md) = send(&r, authed("GET", "/v1/pack?budget=2000", Body::empty())).await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            md.contains("Use a peg") && md.contains("ctx-claims"),
            "{md}"
        );

        let (_, found) = send(&r, authed("GET", "/v1/search?q=peg", Body::empty())).await;
        assert!(found.contains("Use a peg"));
        let (_, b) = send(&r, authed("GET", "/v1/branches", Body::empty())).await;
        assert!(b.contains("\"active\":\"default\""), "{b}");
    }
}
