use std::env::home_dir;
use std::path::{Path, PathBuf};
use std::{collections::HashMap, fs, fs::File, sync::Arc};

use anyhow::Result;
use axum::body::Bytes;
use axum::{
    extract::{Query, State},
    http::{Method, StatusCode},
    response::Html,
    routing::{get, get_service, post},
    Json, Router,
};

use ruggle_engine::search::{Hit, Scope};
use ruggle_engine::types::CrateMetadata;
use ruggle_server::{
    index_local_crate, make_index, make_sets, perform_search, pull_crate_from_remote_index,
    pull_set_from_remote_index, Scopes,
};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;
use tokio::sync::{Notify, RwLock};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};

use ruggle_engine::Index;
use ruggle_engine::{build_parent_index, types};

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
        &state.index,
        &state.scopes,
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
        &state.index,
        &state.scopes,
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
    let index_dir: PathBuf = opt.index.unwrap_or_else(|| {
        PathBuf::from(home_dir().unwrap_or_else(|| PathBuf::from("."))).join(".ruggle")
    });
    let index = make_index(&index_dir).await.expect("failed to build index");
    let sets = make_sets(Path::new(&index_dir));
    let krates = index
        .crates
        .keys()
        .map(|k| (k.clone(), Scope::Crate(k.clone())))
        .collect();
    let scopes = ruggle_server::Scopes { sets, krates };
    let shutdown_notify = Arc::new(Notify::new());
    let state = Arc::new(RwLock::new(AppState {
        index,
        scopes,
        shutdown: shutdown_notify.clone(),
        index_dir: index_dir.clone(),
    }));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let static_service = get_service(ServeDir::new(STATIC_DIR))
        .handle_error(|e| async move { (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")) });

    let app = Router::new()
        .route("/index", get(index_get).post(update_index))
        .route("/index/local", post(update_local_index))
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
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_file(true)
        .with_line_number(true)
        .without_time()
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
async fn index_get(State(state): State<Arc<RwLock<AppState>>>) -> Json<Vec<CrateMetadata>> {
    let state = state.read().await;
    let mut metadata: Vec<CrateMetadata> = state.index.crates.keys().cloned().collect();
    tracing::info!("returning {} indexed crates", metadata.len());
    metadata.sort();
    tracing::debug!("indexed crates: {:?}", metadata);
    Json(metadata)
}

#[derive(Deserialize)]
struct IndexRequest {
    scopes: Vec<Scope>,
}

/// Update the in-memory index by fetching one or more crate JSON/bin files.
/// Example body: {"urls": ["https://raw.githubusercontent.com/alpaylan/ruggle-index/main/crate/std.json"]}
async fn update_index(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<IndexRequest>,
) -> Result<Json<String>, StatusCode> {
    tracing::debug!("update_index request: {:?}", req.scopes);
    if req.scopes.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut updated = 0usize;
    for scope in req.scopes {
        let krates = match scope {
            Scope::Crate(krate) => vec![krate],
            Scope::Set(scope, _) => {
                let krates = pull_set_from_remote_index(&scope).await.map_err(|e| {
                    tracing::error!("pulling set `{}` failed: {}", scope, e);
                    StatusCode::BAD_GATEWAY
                })?;
                {
                    state
                        .write()
                        .await
                        .scopes
                        .sets
                        .insert(scope.clone(), Scope::Set(scope, krates.clone()));
                }
                krates
            }
        };

        for metadata in krates {
            let krate = pull_crate_from_remote_index(&metadata).await.map_err(|e| {
                tracing::error!("pulling crate `{}` failed: {}", metadata, e);
                StatusCode::BAD_GATEWAY
            })?;
            // Build parent index
            let parents = build_parent_index(&krate);
            // Persist as .bin under <index_dir>/crate/<name>.bin
            {
                let state_read = state.read().await;
                let crate_dir = state_read.index_dir.join("crate");
                let _ = fs::create_dir_all(&crate_dir);
                tracing::debug!("created crate directory: {}", crate_dir.display());

                let mut file =
                    File::create(crate_dir.join(format!("{}.bin", metadata))).map_err(|e| {
                        tracing::error!("failed creating crate file for {}: {}", metadata, e);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;
                tracing::debug!(
                    "created crate file: {}",
                    crate_dir.join(format!("{}.bin", metadata)).display()
                );
                bincode::encode_into_std_write(&krate, &mut file, bincode::config::standard())
                    .map_err(|e| {
                        tracing::error!("failed writing crate file for {}: {}", metadata, e);
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;

                let mut parents_file = File::create(
                    crate_dir.join(format!("{}.parents.bin", metadata)),
                )
                .map_err(|e| {
                    tracing::error!("failed creating parents file for {}: {}", metadata, e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
                bincode::encode_into_std_write(
                    &parents,
                    &mut parents_file,
                    bincode::config::standard(),
                )
                .map_err(|e| {
                    tracing::error!("failed writing parents file for {}: {}", metadata, e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            }
            // Update in-memory index
            {
                let mut state_write = state.write().await;
                state_write.index.crates.insert(metadata.clone(), krate);
                state_write.index.parents.insert(metadata.clone(), parents);
                state_write
                    .scopes
                    .krates
                    .insert(metadata.clone(), Scope::Crate(metadata));
            }
            updated += 1;
        }
    }
    Ok(Json(format!("updated {} crates", updated)))
}

#[derive(Deserialize)]
struct LocalIndexRequest {
    cargo_manifest_path: PathBuf,
}

async fn update_local_index(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<LocalIndexRequest>,
) -> Result<Json<String>, StatusCode> {
    // Verify that the path is `Cargo.toml`
    if !req
        .cargo_manifest_path
        .file_name()
        .map(|f| f == "Cargo.toml")
        .unwrap_or(false)
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let crates: Vec<types::Crate> = {
        let mut state = state.write().await;
        index_local_crate(&mut state.index, &req.cargo_manifest_path)
            .await
            .map_err(|e| {
                tracing::error!("local index error: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };
    // Persist the crates
    for krate in &crates {
        let crate_dir = state.read().await.index_dir.join("crate");
        let _ = fs::create_dir_all(&crate_dir);
        let mut file =
            File::create(crate_dir.join(format!("{}.bin", krate.name.clone().unwrap_or_default())))
                .map_err(|e| {
                    tracing::error!(
                        "failed creating crate file for {}: {}",
                        krate.name.clone().unwrap_or_default(),
                        e
                    );
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        bincode::encode_into_std_write(krate, &mut file, bincode::config::standard()).map_err(
            |e| {
                tracing::error!(
                    "failed writing crate file for {}: {}",
                    krate.name.clone().unwrap_or_default(),
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            },
        )?;
    }

    let parents = crates
        .iter()
        .map(|krate| {
            (
                krate.name.clone().expect("crate SHOULD HAVE a name"),
                build_parent_index(krate),
            )
        })
        .collect::<HashMap<_, _>>();

    // Persist the parents
    for (name, parents) in parents.iter() {
        let crate_dir = state.read().await.index_dir.join("crate");
        let _ = fs::create_dir_all(&crate_dir);
        let mut parents_file = File::create(crate_dir.join(format!("{}.parents.bin", name)))
            .map_err(|e| {
                tracing::error!("failed creating parents file for {}: {}", name, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        bincode::encode_into_std_write(parents, &mut parents_file, bincode::config::standard())
            .map_err(|e| {
                tracing::error!("failed writing parents file for {}: {}", name, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    let mut state = state.write().await;
    let mut metadatas_for_set: Vec<CrateMetadata> = Vec::new();
    for krate in crates {
        let name = krate.name.clone().expect("crate SHOULD HAVE a name");
        let metadata = CrateMetadata {
            name: name.clone(),
            version: krate.crate_version.clone(),
        };
        state.index.crates.insert(metadata.clone(), krate);
        state.index.parents.insert(
            metadata.clone(),
            parents
                .get(&name)
                .cloned()
                .expect("crates index SHOULD BE in sync with the parents index"),
        );
        // Register individual crate scopes for convenience
        state
            .scopes
            .krates
            .insert(metadata.clone(), Scope::Crate(metadata.clone()));
        metadatas_for_set.push(metadata);
    }

    // Create a new Set for this local project to make scope switching easy
    let set_name = req
        .cargo_manifest_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| format!("local-{}", s))
        .unwrap_or_else(|| "local".to_string());

    state.scopes.sets.insert(
        set_name.clone(),
        Scope::Set(set_name.clone(), metadatas_for_set.clone()),
    );

    // Persist the set so it shows up on restart as well
    let set_dir = state.index_dir.join("set");
    let _ = fs::create_dir_all(&set_dir);
    if let Ok(json) = serde_json::to_string(&metadatas_for_set) {
        let path = set_dir.join(format!("{}.json", set_name));
        if let Err(e) = std::fs::write(&path, json) {
            tracing::warn!(
                "failed to persist set {} to {}: {}",
                set_name,
                path.display(),
                e
            );
        }
    } else {
        tracing::warn!("failed to serialize set {} for persistence", set_name);
    }

    Ok(Json(format!(
        "updated {} crates; created set:{}",
        state.index.crates.len(),
        set_name
    )))
}
