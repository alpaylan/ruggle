use std::path::{Path, PathBuf};
use std::{collections::HashMap, fs, fs::File, sync::Arc};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use axum::{
    extract::{Query, State},
    http::{Method, StatusCode},
    response::Html,
    routing::{get, get_service, post},
    Json, Router,
};

use roogle_server::make_index;
use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use tokio::sync::{Notify, RwLock};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tracing::{debug, warn};

use roogle_engine::types;
use roogle_engine::{
    query::parse::parse_query,
    search::{Hit, Scope},
    Index,
};

const STATIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/static");

struct AppState {
    index: Index,
    scopes: Scopes,
    shutdown: Arc<Notify>,
    index_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    scope: String,
    query: Option<String>,
    limit: Option<usize>,
    threshold: Option<f32>,
}

async fn search_get(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Hit>>, (StatusCode, String)> {
    let query_str = params
        .query
        .as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "missing query".to_string()))?;
    let state = state.read().await;
    perform_search(
        &state,
        query_str,
        &params.scope,
        params.limit,
        params.threshold,
    )
    .map(Json)
    .map_err(|e| {
        tracing::error!("search error: {}", e);
        internal_or_bad_request(e)
    })
}

async fn search_post(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(mut params): Query<SearchParams>,
    body: Bytes,
) -> Result<Json<Vec<Hit>>, (StatusCode, String)> {
    let body_str = String::from_utf8(body.to_vec()).unwrap_or_default();
    if params.query.is_none() && !body_str.is_empty() {
        params.query = Some(body_str);
    }
    let query_str = params
        .query
        .as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "missing query".to_string()))?;
    let state = state.read().await;
    perform_search(
        &state,
        query_str,
        &params.scope,
        params.limit,
        params.threshold,
    )
    .map(Json)
    .map_err(internal_or_bad_request)
}

fn internal_or_bad_request(e: anyhow::Error) -> (StatusCode, String) {
    // Heuristically classify some errors as bad request
    let msg = format!("{}", e);
    if msg.contains("parsing scope") || msg.contains("parsing query") {
        (StatusCode::BAD_REQUEST, msg)
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, msg)
    }
}

fn perform_search(
    state: &AppState,
    query_str: &str,
    scope_str: &str,
    limit: Option<usize>,
    threshold: Option<f32>,
) -> anyhow::Result<Vec<Hit>> {
    let scope = match scope_str.split(':').collect::<Vec<_>>().as_slice() {
        ["set", set] => state
            .scopes
            .sets
            .get(*set)
            .context(format!("set `{}` not found", set))?,
        ["crate", krate] => state
            .scopes
            .krates
            .get(*krate)
            .context(format!("krate `{}` not found", krate))?,
        _ => Err(anyhow!("parsing scope `{}` failed", scope_str))?,
    };
    debug!(?scope);

    let query = parse_query(query_str)
        .ok()
        .context(format!("parsing query `{}` failed", query_str))?
        .1;
    debug!(?query);

    let limit = limit.unwrap_or(30);
    let threshold = threshold.unwrap_or(0.4);

    let hits = state
        .index
        .search(&query, scope.clone(), threshold)
        .with_context(|| format!("search with query `{:?}` failed", query))?;
    let hits = hits
        .into_iter()
        .inspect(|hit| debug!(?hit.name, link = ?hit.link, similarities = ?hit.similarities(), score = ?hit.similarities().score()))
        .take(limit)
        .collect::<Vec<_>>();

    Ok(hits)
}

async fn scopes_handler(State(state): State<Arc<RwLock<AppState>>>) -> Json<Vec<String>> {
    let state = state.read().await;
    let mut result = vec![];
    for set in state.scopes.sets.keys() {
        result.push(format!("set:{}", set));
    }
    for krate in state.scopes.krates.keys() {
        result.push(format!("crate:{}", krate));
    }
    Json(result)
}

#[derive(Debug, StructOpt, Deserialize)]
struct Opt {
    #[structopt(short, long, name = "INDEX")]
    index: Option<PathBuf>,
    #[structopt(long, default_value = "127.0.0.1")]
    host: String,
    #[structopt(long, default_value = "8000")]
    port: u16,
    /// Optional file path to write the selected listening URL as JSON {"url":"http://host:port"}
    #[structopt(long, name = "PORT_FILE")]
    port_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    init_logger();

    let opt = Opt::from_args();
    let index_dir: PathBuf = opt
        .index
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../roogle-index")));
    let index = make_index(&index_dir).expect("failed to build index");
    let scopes = make_scopes(&index_dir).expect("failed to build scopes");
    let shutdown_notify = Arc::new(Notify::new());
    let state = Arc::new(RwLock::new(AppState {
        index,
        scopes,
        shutdown: shutdown_notify.clone(),
        index_dir: index_dir.clone(),
    }));
    // By default add `set:libstd`
    state.write().await.scopes.sets.insert(
        "libstd".to_string(),
        Scope::Set(
            "libstd".to_string(),
            vec!["std".to_string(), "core".to_string(), "alloc".to_string()],
        ),
    );

    state
        .write()
        .await
        .scopes
        .krates
        .insert("std".to_string(), Scope::Crate("std".to_string()));
    state
        .write()
        .await
        .scopes
        .krates
        .insert("core".to_string(), Scope::Crate("core".to_string()));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let static_service = get_service(ServeDir::new(STATIC_DIR))
        .handle_error(|e| async move { (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")) });

    let app = Router::new()
        .route("/index", get(index_get).post(update_index))
        .route("/search", get(search_get).post(search_post))
        .route("/healthz", get(healthz))
        .route("/stop", post(stop))
        .route("/scopes", get(scopes_handler))
        .route("/", get(index_page))
        .nest_service("/static", static_service)
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Bind, supporting port 0 to request an ephemeral port
    let bind_host: std::net::IpAddr = opt
        .host
        .parse()
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let addr = std::net::SocketAddr::from((bind_host, opt.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {}: {}", addr, e));
    let local_addr = listener.local_addr().expect("local_addr");
    tracing::info!("listening on http://{}", local_addr);

    // Optional: write port-file with the resolved URL
    if let Some(port_file) = &opt.port_file {
        if let Some(parent) = port_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let url = format!("http://{}", local_addr);
        let body = serde_json::json!({"url": url});
        if let Err(e) = std::fs::write(port_file, body.to_string()) {
            tracing::warn!("failed writing port file {:?}: {}", port_file, e);
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown_notify.notified().await })
        .await
        .unwrap();
}

async fn index_page() -> Result<Html<String>, (StatusCode, String)> {
    let html = tokio::fs::read_to_string(format!("{}/index.html", STATIC_DIR))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Html(html))
}

fn init_logger() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = match std::env::var("ROOGLE_LOG") {
        Ok(env) => EnvFilter::new(env),
        _ => return,
    };
    fmt()
        .with_env_filter(filter)
        .pretty()
        .with_target(true)
        .init();
}

#[derive(Serialize)]
struct Healthz {
    status: &'static str,
    version: &'static str,
}

async fn healthz() -> Json<Healthz> {
    Json(Healthz {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn stop(State(state): State<Arc<RwLock<AppState>>>) -> StatusCode {
    let state = state.read().await;
    state.shutdown.notify_waiters();
    StatusCode::OK
}

/// Return the list of currently indexed crate names (in-memory index keys).
async fn index_get(State(state): State<Arc<RwLock<AppState>>>) -> Json<Vec<String>> {
    let state = state.read().await;
    let mut names: Vec<String> = state.index.crates.keys().cloned().collect();
    names.sort();
    Json(names)
}

#[derive(Deserialize)]
struct IndexRequest {
    scopes: Vec<Scope>,
}

/// Update the in-memory index by fetching one or more crate JSON/bin files.
/// Example body: {"urls": ["https://raw.githubusercontent.com/alpaylan/roogle-index/main/crate/std.json"]}
async fn update_index(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<IndexRequest>,
) -> Result<Json<String>, StatusCode> {
    if req.scopes.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let client = reqwest::Client::new();
    let mut updated = 0usize;
    for scope in req.scopes {
        let url = scope.url();
        let urls = match scope {
            Scope::Crate(_) => vec![url],
            Scope::Set(scope, _) => {
                // fetch the original URL of the set to get the list of crates
                tracing::debug!("Fetching set URL '{url}'");
                let res = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|_| StatusCode::BAD_GATEWAY)?;
                // Parse as `Vec<String>`
                let mut crates = vec![];
                if res.status().is_success() {
                    let bytes = res.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
                    crates = serde_json::from_slice::<Vec<String>>(&bytes)
                        .map_err(|_| StatusCode::BAD_GATEWAY)?
                }
                {
                    state
                        .write()
                        .await
                        .scopes
                        .sets
                        .insert(scope.clone(), Scope::Set(scope, crates.clone()));
                }
                crates.into_iter().map(|s| Scope::Crate(s).url()).collect()
            }
        };
        for url in urls {
            tracing::info!("fetching index url={}", url);
            let res = client
                .get(&url)
                .send()
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            if !res.status().is_success() {
                continue;
            }
            let bytes = res.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;

            if let Ok((mut krate, _len)) =
                bincode::decode_from_slice::<types::Crate, _>(&bytes, bincode::config::standard())
            {
                // derive crate name from URL (last segment without extension)
                let name = url
                    .split('/')
                    .next_back()
                    .and_then(|seg| seg.split('.').next())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    tracing::warn!("skipping crate with empty name from {}", url);
                    continue;
                } else {
                    krate.name = Some(name.clone());
                }
                // Persist as .bin under <index_dir>/crate/<name>.bin
                {
                    let state_read = state.read().await;
                    let crate_dir = state_read.index_dir.join("crate");
                    let _ = fs::create_dir_all(&crate_dir);
                    let mut file = File::create(crate_dir.join(format!("{}.bin", name)))
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    bincode::encode_into_std_write(&krate, &mut file, bincode::config::standard())
                        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                }
                // Update in-memory index
                {
                    let mut state_write = state.write().await;
                    state_write.index.crates.insert(name, krate);
                }
                updated += 1;
            }
        }
    }
    Ok(Json(format!("updated {} crates", updated)))
}

struct Scopes {
    sets: HashMap<String, Scope>,
    krates: HashMap<String, Scope>,
}

fn make_scopes(index_dir: &Path) -> Result<Scopes> {
    let krates: HashMap<String, Scope> =
        std::fs::read_dir(format!("{}/crate", index_dir.display()))
            .context("failed to read crate files")?
            .map(|entry| {
                let entry = entry?;
                let path = entry.path();
                let krate = path.file_stem().unwrap().to_str().unwrap(); // SAFETY: files in `roogle-index` has a name.

                Ok((krate.to_owned(), Scope::Crate(krate.to_owned())))
            })
            .filter_map(|res: Result<_, anyhow::Error>| {
                if let Err(ref e) = res {
                    warn!("registering a scope skipped: {}", e)
                }
                res.ok()
            })
            .collect();
    let sets: HashMap<String, Scope> =
        match std::fs::read_dir(format!("{}/set", index_dir.display())) {
            Err(e) => {
                warn!("registering sets skipped: {}", e);
                HashMap::default()
            }
            Ok(entry) => {
                entry
                    .map(|entry| {
                        let entry = entry?;
                        let path = entry.path();
                        let json = std::fs::read_to_string(&path)
                            .context(format!("failed to read `{:?}`", path))?;
                        let set = path.file_stem().unwrap().to_str().unwrap().to_owned(); // SAFETY: files in `roogle-index` has a name.
                        let krates = serde_json::from_str::<Vec<String>>(&json)
                            .context(format!("failed to deserialize set `{}`", &set))?;

                        Ok((set.clone(), Scope::Set(set, krates)))
                    })
                    .filter_map(|res: Result<_, anyhow::Error>| {
                        if let Err(ref e) = res {
                            warn!("registering a scope skipped: {}", e)
                        }
                        res.ok()
                    })
                    .collect()
            }
        };
    Ok(Scopes { sets, krates })
}
