use std::path::{Path, PathBuf};

use anyhow::Context as _;
use anyhow::{anyhow, Result};
use roogle_engine::{query::parse::parse_query, search::Hit, Index};
use roogle_server::{generate_bin_index, make_index, make_scopes, shake_index, Scopes};

use structopt::StructOpt;
use tracing::{debug, info};

#[derive(Debug, StructOpt)]
struct Cli {
    /// Roogle server base URL
    #[structopt(long, default_value = "http://localhost:8000")]
    host: String,

    /// Path to roogle-index directory
    /// If omitted, use `../roogle-index` relative to this binary.
    #[structopt(long, parse(from_os_str))]
    index: Option<PathBuf>,
    /// Scope string like set:libstd or crate:std
    #[structopt(long)]
    scope: String,

    /// Result limit
    #[structopt(long, default_value = "30")]
    limit: usize,

    /// Threshold (0.0-1.0)
    #[structopt(long, default_value = "0.4")]
    threshold: f32,

    /// Output as JSON
    #[structopt(long)]
    json: bool,

    /// Query string; if omitted, read from stdin
    query: Option<String>,

    /// Ask to the server instead of local index
    /// This requires the `host` to be set properly.
    #[structopt(long)]
    server: bool,

    /// Shake the index files under the given `index` directory
    /// This modifies the index files in-place.
    #[structopt(long)]
    shake: bool,

    /// Generate binary index files under the given `index` directory
    /// This writes `.bin` files alongside the original `.json` files.
    #[structopt(long)]
    binary: bool,
}

fn perform_search(
    index: Index,
    scopes: Scopes,
    query_str: &str,
    scope_str: &str,
    limit: Option<usize>,
    threshold: Option<f32>,
) -> anyhow::Result<Vec<Hit>> {
    let scope = match scope_str.split(':').collect::<Vec<_>>().as_slice() {
        ["set", set] => scopes
            .sets
            .get(*set)
            .context(format!("set `{}` not found", set))?,
        ["crate", krate] => scopes
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

    let hits = index
        .search(&query, scope.clone(), threshold)
        .with_context(|| format!("search with query `{:?}` failed", query))?;
    let hits = hits
        .into_iter()
        .inspect(|hit| debug!(?hit.name, ?hit.link, similarities = ?hit.similarities(), score = ?hit.similarities().score()))
        .take(limit)
        .collect::<Vec<_>>();

    Ok(hits)
}

async fn ask_server(
    host: &str,
    scope: &str,
    query: &str,
    limit: usize,
    threshold: f32,
) -> Result<Vec<Hit>> {
    let client = reqwest::Client::new();
    tracing::debug!("(scope={}, query={})", scope, query);
    let url = format!(
        "{}/search?scope={}&query={}&limit={}&threshold={}",
        host,
        urlencoding::encode(scope),
        urlencoding::encode(&query),
        limit,
        threshold
    );
    tracing::debug!("requesting {}", url);

    let res = client.get(&url).send().await.context("request failed")?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        anyhow::bail!("{}: {}", status, text);
    }

    let hits: Vec<Hit> = res.json().await.context("invalid response body")?;

    Ok(hits)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::from_args();
    let query = match cli.query {
        Some(q) => q,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };

    let index_dir = cli
        .index
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../roogle-index")));

    if cli.shake {
        shake_index(&index_dir).context("failed to shake index")?;
        info!("index shaken successfully");
        return Ok(());
    }

    if cli.binary {
        generate_bin_index(&index_dir).context("failed to generate binary index")?;
        info!("binary index generated successfully");
        return Ok(());
    }

    let hits = if cli.server {
        ask_server(&cli.host, &cli.scope, &query, cli.limit, cli.threshold).await?
    } else {
        let index = make_index(&index_dir).expect("failed to build index");
        tracing::info!("index built successfully");
        let scopes = make_scopes(Path::new(&index_dir)).context("failed to make scopes")?;
        tracing::info!("scopes created successfully");
        perform_search(
            index,
            scopes,
            &query,
            &cli.scope,
            Some(cli.limit),
            Some(cli.threshold),
        )?
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }

    for (i, h) in hits.iter().enumerate() {
        let path = h.path.join("::");
        let link = format!("https://doc.rust-lang.org/{}", h.link.join("/"));
        println!("{:>2}. {}  ({})\n    {}", i + 1, h.name, path, link);
    }

    Ok(())
}
