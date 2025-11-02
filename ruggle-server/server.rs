use std::env::home_dir;
use std::path::{Path, PathBuf};
use std::{
    collections::HashMap,
    fs,
    fs::File,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use axum::body::Bytes;
use axum::{
    extract::{Query, State},
    http::{Method, StatusCode},
    response::Html,
    routing::{get, get_service, post},
    Json, Router,
};

use ruggle_engine::search::{Hit, Scope, Set};
use ruggle_engine::types::{CrateMetadata, Item};
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

use ruggle_engine::compare::Similarity;
use ruggle_engine::query::parse::parse_query;
use ruggle_engine::Index;
use ruggle_engine::Path as DocPath;
use ruggle_engine::{build_parent_index, types};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{self as ts, Layer as _};

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
    for krate in state.scopes.krates.iter() {
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
        home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ruggle")
    });
    let index = make_index(&index_dir).await.expect("failed to build index");
    let sets = make_sets(Path::new(&index_dir));
    let krates = index.crates.keys().cloned().collect();
    let scopes = Scopes { sets, krates };
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
        .route("/debug/functions", get(debug_functions_handler))
        .route("/debug/similarity", get(debug_similarity_handler))
        .route("/debug/query", get(debug_query_handler))
        .route("/debug/compare_logs", get(debug_compare_logs_handler))
        .route("/debug/doc", get(debug_doc_handler))
        .route("/debug/parents", get(debug_parents_handler))
        .route("/debug/types", get(debug_types_handler))
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
    let html = include_bytes!("./static/index.html");
    let html = String::from_utf8(html.to_vec()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load index page: {}", e),
        )
    })?;
    Ok(Html(html))
}

fn init_logger() {
    use tracing_subscriber::{
        filter::{LevelFilter, Targets},
        fmt,
        layer::SubscriberExt,
        util::SubscriberInitExt,
        EnvFilter,
    };

    // Console (env-controlled) layer (no ANSI)
    let console_layer = fmt::layer()
        .with_ansi(true)
        .with_file(true)
        .with_line_number(true)
        .without_time();

    // File layer: always TRACE, non-ANSI, to debug.log
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("debug.log")
        .unwrap_or_else(|e| panic!("failed to open debug.log: {}", e));
    let file_mw = LogFileMakeWriter(Arc::new(Mutex::new(file)));
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .without_time()
        .with_writer(file_mw);

    // Limit file logs to our crates
    let our_targets = Targets::new()
        .with_target("ruggle_server", LevelFilter::TRACE)
        .with_target("ruggle_engine", LevelFilter::TRACE);

    tracing_subscriber::registry()
        .with(console_layer.with_filter(EnvFilter::from_default_env()))
        .with(file_layer.with_filter(our_targets))
        .init();
}

// Simple file MakeWriter for the non-ANSI log file
struct LogFileWriter(Arc<Mutex<File>>);
impl std::io::Write for LogFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = self.0.lock().unwrap();
        g.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct LogFileMakeWriter(Arc<Mutex<File>>);
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFileMakeWriter {
    type Writer = LogFileWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogFileWriter(self.0.clone())
    }
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
            Scope::Set(scope) => {
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
                        .insert(scope.clone(), Set::new(scope, krates.clone()));
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
                state_write.scopes.krates.insert(metadata);
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
        state.scopes.krates.insert(metadata.clone());
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
        Set::new(set_name.clone(), metadatas_for_set.clone()),
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

#[derive(Debug, Deserialize)]
struct DebugFunctionParams {
    scope: String,
}

async fn debug_functions_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugFunctionParams>,
) -> Result<Json<Vec<Item>>, (StatusCode, String)> {
    let scope = Scope::try_from(params.scope.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("parsing scope `{}` failed: {}", params.scope, e),
        )
    })?;
    let state = state.read().await;
    let krates = state.scopes.get(&scope).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("resolving scope `{}` failed: {}", params.scope, e),
        )
    })?;

    let mut results = vec![];
    for krate in krates {
        let krate = state.index.crates.get(&krate).ok_or((
            StatusCode::BAD_REQUEST,
            format!("crate `{}` not found in index", krate),
        ))?;
        results.extend(
            krate
                .index
                .iter()
                .filter(|(_, item)| matches!(item.inner, types::ItemEnum::Function(_)))
                .map(|(_, item)| item)
                .cloned(),
        );
    }
    Ok(Json(results))
}

#[derive(Debug, Deserialize)]
struct DebugQueryParams {
    query: String,
}

async fn debug_query_handler(
    Query(params): Query<DebugQueryParams>,
) -> Result<Json<ruggle_engine::query::Query>, (StatusCode, String)> {
    let parsed = parse_query(params.query.as_str())
        .map(|(_, q)| q)
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                format!("parsing query `{}` failed", params.query),
            )
        })?;
    Ok(Json(parsed))
}

#[derive(Debug, Deserialize)]
struct DebugSimilarityParams {
    scope: String,
    query: String,
    id: u32,
}

#[derive(Debug, Serialize)]
struct PartJson {
    discrete: Option<&'static str>,
    continuous: Option<f32>,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct SimilarityJson {
    score: f32,
    parts: Vec<PartJson>,
}

async fn debug_similarity_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugSimilarityParams>,
) -> Result<Json<SimilarityJson>, (StatusCode, String)> {
    let scope = Scope::try_from(params.scope.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("parsing scope `{}` failed: {}", params.scope, e),
        )
    })?;
    let query = parse_query(params.query.as_str())
        .ok()
        .map(|(_, q)| q)
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("parsing query `{}` failed", params.query),
        ))?;

    let state = state.read().await;
    let krates = state.scopes.get(&scope).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("resolving scope `{}` failed: {}", params.scope, e),
        )
    })?;

    // Find the item by id in the given scope's crates.
    let mut found: Option<(types::Item, &types::Crate)> = None;
    for km in &krates {
        if let Some(krate) = state.index.crates.get(km) {
            if let Some(item) = krate.index.get(&types::Id(params.id)) {
                found = Some((item.clone(), krate));
                break;
            }
        }
    }

    let (item, krate) = found.ok_or((
        StatusCode::NOT_FOUND,
        format!(
            "item with id {} not found in scope {}",
            params.id, params.scope
        ),
    ))?;

    let sims = state.index.compare(&query, &item, krate, None);
    let score = sims.score();
    let parts = sims
        .0
        .into_iter()
        .map(|s| match s {
            Similarity::Discrete { kind, reason } => {
                let label: &'static str = match kind {
                    ruggle_engine::compare::DiscreteSimilarity::Equivalent => "equivalent",
                    ruggle_engine::compare::DiscreteSimilarity::Subequal => "subequal",
                    ruggle_engine::compare::DiscreteSimilarity::Different => "different",
                };
                PartJson {
                    discrete: Some(label),
                    continuous: None,
                    reason: Some(reason),
                }
            }
            Similarity::Continuous { value, reason } => PartJson {
                discrete: None,
                continuous: Some(value),
                reason: Some(reason),
            },
        })
        .collect::<Vec<_>>();

    Ok(Json(SimilarityJson { score, parts }))
}

#[derive(Debug, Deserialize)]
struct DebugDocParams {
    scope: String,
    id: u32,
}

#[derive(Debug, Serialize)]
struct DocJson {
    link: String,
    path: Vec<String>,
}

async fn debug_doc_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugDocParams>,
) -> Result<Json<DocJson>, (StatusCode, String)> {
    let scope = Scope::try_from(params.scope.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("parsing scope `{}` failed: {}", params.scope, e),
        )
    })?;
    let state = state.read().await;
    let krates = state.scopes.get(&scope).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("resolving scope `{}` failed: {}", params.scope, e),
        )
    })?;

    // Find the item and its crate metadata
    let mut found: Option<(
        types::Item,
        &types::Crate,
        &ruggle_engine::types::CrateMetadata,
    )> = None;
    for km in &krates {
        if let Some(krate) = state.index.crates.get(km) {
            if let Some(item) = krate.index.get(&types::Id(params.id)) {
                found = Some((item.clone(), krate, km));
                break;
            }
        }
    }
    let (item, krate, km) = found.ok_or((
        StatusCode::NOT_FOUND,
        format!(
            "item with id {} not found in scope {}",
            params.id, params.scope
        ),
    ))?;

    // Reconstruct path from parents index
    let parents = state.index.parents.get(km).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "parents not found".to_string(),
    ))?;

    let mut path = DocPath {
        name: krate.name.clone().unwrap_or_default(),
        modules: vec![],
        owner: None,
        item: item.clone(),
    };

    let mut walker = Some(item.id);
    while let Some(here) = walker {
        match parents.get(&here) {
            Some(ruggle_engine::Parent::Module(mid)) => {
                let mi = &krate.index[mid];
                if let types::ItemEnum::Module(m) = &mi.inner {
                    if m.is_crate {
                        path.modules.push(mi.clone());
                        break;
                    }
                }
                if mi.name.is_some() {
                    path.modules.push(mi.clone());
                }
                walker = Some(*mid);
            }
            Some(ruggle_engine::Parent::Trait(tid))
            | Some(ruggle_engine::Parent::Impl(tid))
            | Some(ruggle_engine::Parent::Struct(tid)) => {
                path.owner = Some(krate.index.get(tid).unwrap().clone());
                walker = Some(*tid);
            }
            None => break,
        }
    }
    path.modules.reverse();

    let link = path.link();
    let path_vec = path.pathify();
    Ok(Json(DocJson {
        link,
        path: path_vec,
    }))
}

#[derive(Debug, Deserialize)]
struct DebugTypesParams {
    scope: String,
}

#[derive(Debug, Serialize)]
struct TypeEntryJson {
    name: String,
    kind: String,
    path: Vec<String>,
    link: String,
}

/// Return concrete types within the given scope (structs, enums, unions, type aliases, primitives)
async fn debug_types_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugTypesParams>,
) -> Result<Json<Vec<TypeEntryJson>>, (StatusCode, String)> {
    let scope = Scope::try_from(params.scope.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("parsing scope `{}` failed: {}", params.scope, e),
        )
    })?;
    let state = state.read().await;
    let krates = state.scopes.get(&scope).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("resolving scope `{}` failed: {}", params.scope, e),
        )
    })?;

    let mut out = Vec::new();
    for km in &krates {
        let krate = state.index.crates.get(km).ok_or((
            StatusCode::BAD_REQUEST,
            format!("crate `{}` not found in index", km),
        ))?;
        let parents = state.index.parents.get(km).ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "parents missing".to_string(),
        ))?;

        for (id, item) in krate.index.iter() {
            let kind = match &item.inner {
                types::ItemEnum::Struct(_) => Some("struct"),
                types::ItemEnum::Enum(_) => Some("enum"),
                types::ItemEnum::Union(_) => Some("union"),
                types::ItemEnum::TypeAlias(_) => Some("type_alias"),
                types::ItemEnum::Primitive(_) => Some("primitive"),
                _ => None,
            };
            if let Some(kind) = kind {
                // Reconstruct module path for item
                tracing::info!("reconstructing path for item {:?}", item);

                if let Some(p) = ruggle_engine::reconstruct_path_for_local(krate, id, parents) {
                    let path_vec = p.pathify();
                    // Build docs link
                    let crate_name = krate.name.clone().unwrap_or_default();
                    let mut link =
                        if crate_name == "std" || crate_name == "core" || crate_name == "alloc" {
                            "https://doc.rust-lang.org/".to_string()
                        } else {
                            format!("https://docs.rs/{}/latest/", crate_name)
                        };
                    if path_vec.len() > 1 {
                        for seg in &path_vec[..path_vec.len() - 1] {
                            link.push_str(seg);
                            link.push('/');
                        }
                    }
                    let iname = item.name.clone().unwrap_or_default();
                    let suffix = match kind {
                        "struct" => format!("struct.{}.html", iname),
                        "enum" => format!("enum.{}.html", iname),
                        "union" => format!("union.{}.html", iname),
                        "type_alias" => format!("type.{}.html", iname),
                        "primitive" => format!("primitive.{}.html", iname),
                        _ => format!("{}.html", iname),
                    };
                    link.push_str(&suffix);

                    out.push(TypeEntryJson {
                        name: iname,
                        kind: kind.to_string(),
                        path: path_vec,
                        link,
                    });
                }
            }
        }
    }

    Ok(Json(out))
}

// Parents/graph explorer (restored)
#[derive(Debug, Serialize)]
struct GraphNodeJson {
    id: u32,
    name: Option<String>,
    kind: String,
}

#[derive(Debug, Serialize)]
struct GraphEdgeJson {
    from: u32,
    to: u32,
    relation: String,
}

#[derive(Debug, Serialize)]
struct GraphJson {
    krate: CrateMetadata,
    nodes: Vec<GraphNodeJson>,
    edges: Vec<GraphEdgeJson>,
}

#[derive(Debug, Deserialize)]
struct DebugParentsParams {
    #[serde(rename = "crate")]
    krate: String,
}

async fn debug_parents_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugParentsParams>,
) -> Result<Json<GraphJson>, (StatusCode, String)> {
    let state = state.read().await;
    // Parse name[:version]
    let (name, version_opt) = match params.krate.split_once(':') {
        Some((n, v)) if !n.is_empty() && !v.is_empty() => (n.to_string(), Some(v.to_string())),
        _ => (params.krate.clone(), None),
    };
    // Pick crate
    let mut selected: Option<CrateMetadata> = None;
    for meta in state.index.crates.keys() {
        if meta.name == name {
            if let Some(v) = &version_opt {
                if &meta.version == v {
                    selected = Some(meta.clone());
                    break;
                }
            } else if selected.is_none() {
                selected = Some(meta.clone());
            }
        }
    }
    let selected = selected.ok_or((
        StatusCode::NOT_FOUND,
        format!("crate `{}` not found", params.krate),
    ))?;

    let krate = state.index.crates.get(&selected).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "crate missing".to_string(),
    ))?;
    let parents = state.index.parents.get(&selected).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "parents missing".to_string(),
    ))?;

    // Nodes
    let mut nodes = Vec::with_capacity(krate.index.len());
    for (id, item) in krate.index.iter() {
        let kind = match &item.inner {
            types::ItemEnum::Module(_) => "module",
            types::ItemEnum::ExternCrate { .. } => "extern_crate",
            types::ItemEnum::Use(_) => "use",
            types::ItemEnum::Union(_) => "union",
            types::ItemEnum::Struct(_) => "struct",
            types::ItemEnum::StructField(_) => "struct_field",
            types::ItemEnum::Enum(_) => "enum",
            types::ItemEnum::Variant(_) => "variant",
            types::ItemEnum::Function(_) => "function",
            types::ItemEnum::Trait(_) => "trait",
            types::ItemEnum::TraitAlias(_) => "trait_alias",
            types::ItemEnum::Impl(_) => "impl",
            types::ItemEnum::TypeAlias(_) => "type_alias",
            types::ItemEnum::Constant { .. } => "constant",
            types::ItemEnum::Static(_) => "static",
            types::ItemEnum::ExternType => "extern_type",
            types::ItemEnum::Macro(_) => "macro",
            types::ItemEnum::ProcMacro(_) => "proc_macro",
            types::ItemEnum::Primitive(_) => "primitive",
            types::ItemEnum::AssocConst { .. } => "assoc_const",
            types::ItemEnum::AssocType { .. } => "assoc_type",
        }
        .to_string();
        nodes.push(GraphNodeJson {
            id: id.0,
            name: item.name.clone(),
            kind,
        });
    }

    // Edges (parent -> child)
    let mut edges = Vec::with_capacity(parents.len());
    for (child, parent) in parents.iter() {
        let (from, relation) = match parent {
            ruggle_engine::Parent::Module(pid) => (pid.0, "module"),
            ruggle_engine::Parent::Struct(pid) => (pid.0, "struct"),
            ruggle_engine::Parent::Trait(pid) => (pid.0, "trait"),
            ruggle_engine::Parent::Impl(pid) => (pid.0, "impl"),
        };
        edges.push(GraphEdgeJson {
            from,
            to: child.0,
            relation: relation.to_string(),
        });
    }

    Ok(Json(GraphJson {
        krate: selected,
        nodes,
        edges,
    }))
}

// Simple in-memory writer to capture tracing output
struct SharedBuffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut g = self.0.lock().unwrap();
        g.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> ts::fmt::MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBufferWriter;
    fn make_writer(&'a self) -> Self::Writer {
        SharedBufferWriter(self.inner.clone())
    }
}

#[derive(Debug, Serialize)]
struct CompareLogsJson {
    score: f32,
    parts: Vec<PartJson>,
    logs: String,
}

#[derive(Debug, Deserialize)]
struct DebugCompareParams {
    scope: String,
    query: String,
    id: u32,
}

async fn debug_compare_logs_handler(
    State(state): State<Arc<RwLock<AppState>>>,
    Query(params): Query<DebugCompareParams>,
) -> Result<Json<CompareLogsJson>, (StatusCode, String)> {
    let scope = Scope::try_from(params.scope.as_str()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("parsing scope `{}` failed: {}", params.scope, e),
        )
    })?;
    let query = parse_query(params.query.as_str())
        .ok()
        .map(|(_, q)| q)
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("parsing query `{}` failed", params.query),
        ))?;

    let state = state.read().await;
    let krates = state.scopes.get(&scope).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("resolving scope `{}` failed: {}", params.scope, e),
        )
    })?;

    // Locate item by id across crates in scope
    let mut found: Option<(types::Item, &types::Crate)> = None;
    for km in &krates {
        if let Some(krate) = state.index.crates.get(km) {
            if let Some(item) = krate.index.get(&types::Id(params.id)) {
                found = Some((item.clone(), krate));
                break;
            }
        }
    }
    let (item, krate) = found.ok_or((
        StatusCode::NOT_FOUND,
        format!(
            "item with id {} not found in scope {}",
            params.id, params.scope
        ),
    ))?;

    // Capture tracing output for the comparison
    let buf = Arc::new(Mutex::new(Vec::new()));
    let make = SharedBuffer { inner: buf.clone() };
    let subscriber = ts::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_line_number(true)
        .with_file(true)
        .without_time()
        .with_span_events(FmtSpan::NONE)
        .with_writer(make)
        .finish();

    let (score, parts) = tracing::subscriber::with_default(subscriber, || {
        let sims = state.index.compare(&query, &item, krate, None);
        let score = sims.score();
        let parts = sims
            .0
            .into_iter()
            .map(|s| match s {
                Similarity::Discrete { kind, reason } => {
                    let label: &'static str = match kind {
                        ruggle_engine::compare::DiscreteSimilarity::Equivalent => "equivalent",
                        ruggle_engine::compare::DiscreteSimilarity::Subequal => "subequal",
                        ruggle_engine::compare::DiscreteSimilarity::Different => "different",
                    };
                    PartJson {
                        discrete: Some(label),
                        continuous: None,
                        reason: Some(reason),
                    }
                }
                Similarity::Continuous { value, reason } => PartJson {
                    discrete: None,
                    continuous: Some(value),
                    reason: Some(reason),
                },
            })
            .collect::<Vec<_>>();
        (score, parts)
    });

    let logs = {
        let g = buf.lock().unwrap();
        String::from_utf8_lossy(&g).to_string()
    };

    Ok(Json(CompareLogsJson { score, parts, logs }))
}
