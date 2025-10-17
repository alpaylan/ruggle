use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use axum::{
    extract::{Query, State},
    http::{Method, StatusCode},
    response::Html,
    routing::{get, get_service},
    Json, Router,
};
use rustdoc_types::Crate;
use serde::Deserialize;
use structopt::StructOpt;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};
use tracing::{debug, warn};

use roogle_engine::{
    query::parse::parse_query,
    search::{Hit, Scope},
    Index,
};
use roogle_util::shake;

const STATIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/static");

struct AppState {
    index: Index,
    scopes: Scopes,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    scope: String,
    query: Option<String>,
    limit: Option<usize>,
    threshold: Option<f32>,
}

async fn search_get(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Hit>>, (StatusCode, String)> {
    let query_str = params
        .query
        .as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "missing query".to_string()))?;
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

async fn search_post(
    State(state): State<Arc<AppState>>,
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
        .inspect(|hit| debug!(?hit.name, ?hit.link, similarities = ?hit.similarities(), score = ?hit.similarities().score()))
        .take(limit)
        .collect::<Vec<_>>();

    Ok(hits)
}

async fn scopes_handler(State(state): State<Arc<AppState>>) -> Json<Vec<String>> {
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
    let state = Arc::new(AppState { index, scopes });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let static_service = get_service(ServeDir::new(STATIC_DIR))
        .handle_error(|e| async move { (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")) });

    let app = Router::new()
        .route("/search", get(search_get).post(search_post))
        .route("/scopes", get(scopes_handler))
        .route("/", get(index_page))
        .nest_service("/static", static_service)
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8000));
    tracing::info!("listening on http://{}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
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

fn make_index(index_dir: &PathBuf) -> Result<Index> {
    let crates = std::fs::read_dir(format!("{}/crate", index_dir.display()))
        .context("failed to read index files")?
        .map(|entry| {
            let entry = entry?;
            let json = std::fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read `{:?}`", entry.file_name()))?;
            let mut deserializer = serde_json::Deserializer::from_str(&json);
            deserializer.disable_recursion_limit();
            let krate = Crate::deserialize(&mut deserializer)
                .with_context(|| format!("failed to deserialize `{:?}`", entry.file_name()))?;
            let file_name = entry
                .path()
                .with_extension("")
                .file_name()
                .with_context(|| format!("failed to get file name from `{:?}`", entry.path()))?
                .to_str()
                .context("failed to get `&str` from `&OsStr`")?
                .to_owned();
            Ok((file_name, shake(krate)))
        })
        .filter_map(|res: Result<_, anyhow::Error>| {
            if let Err(ref e) = res {
                warn!("parsing a JSON file skipped: {}", e);
            }
            res.ok()
        })
        .collect::<HashMap<_, _>>();
    Ok(Index { crates })
}

struct Scopes {
    sets: HashMap<String, Scope>,
    krates: HashMap<String, Scope>,
}

fn make_scopes(index_dir: &PathBuf) -> Result<Scopes> {
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

                        Ok((set, Scope::Set(krates)))
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

// Rocket-specific CORS fairing removed; handled via tower-http CorsLayer.
