use std::path::{Path, PathBuf};

use anyhow::Context as _;
use anyhow::Result;
use ruggle_engine::search::Hit;
use ruggle_server::{generate_bin_index, make_index, make_sets, perform_search, shake_index};

use structopt::StructOpt;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, StructOpt)]
struct Cli {
    /// ruggle server base URL
    #[structopt(long, default_value = "http://localhost:8000")]
    host: String,

    /// Path to ruggle-index directory
    /// If omitted, use `../ruggle-index` relative to this binary.
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

    /// Query string
    #[structopt(long)]
    query: String,

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
        urlencoding::encode(query),
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
    println!("ruggle Client v{}", env!("CARGO_PKG_VERSION"));
    tracing_subscriber::fmt::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_file(true)
        .with_line_number(true)
        .without_time()
        .init();
    println!("Logger initialized");
    let cli = Cli::from_args();
    println!("Arguments parsed: {:?}", cli);

    println!("Searching for: {}", cli.query);

    let index_dir = cli
        .index
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../ruggle-index")));

    if cli.shake {
        shake_index(&index_dir).context("failed to shake index")?;
        info!("index shaken successfully");
        return Ok(());
    }

    if cli.binary {
        info!("generating binary index under {}", index_dir.display());
        generate_bin_index(&index_dir).context("failed to generate binary index")?;
        info!("binary index generated successfully");
        return Ok(());
    }

    let hits = if cli.server {
        ask_server(&cli.host, &cli.scope, &cli.query, cli.limit, cli.threshold).await?
    } else {
        let index = make_index(&index_dir).await.expect("failed to build index");
        tracing::info!("index built successfully");
        let sets = make_sets(Path::new(&index_dir));
        let krates = index.crates.keys().cloned().collect();
        let scopes = ruggle_server::Scopes { sets, krates };

        perform_search(
            &index,
            &scopes,
            &cli.query,
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
        let link = format!("https://doc.rust-lang.org/{}", h.link);
        println!(
            "{:>2}. {} ({})  ({}) ({})\n    {}",
            i + 1,
            h.name,
            h.id.0,
            h.path.join("::"),
            h.signature,
            link
        );
    }

    Ok(())
}
