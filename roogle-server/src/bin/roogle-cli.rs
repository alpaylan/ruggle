use anyhow::{Context, Result};
use roogle_engine::search::Hit;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
struct Cli {
    /// Roogle server base URL
    #[structopt(long, default_value = "http://localhost:8000")]
    host: String,

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

    let client = reqwest::Client::new();
    tracing::debug!("(scope={}, query={})", cli.scope, query);
    let url = format!(
        "{}/search?scope={}&query={}&limit={}&threshold={}",
        cli.host,
        urlencoding::encode(&cli.scope),
        urlencoding::encode(&query),
        cli.limit,
        cli.threshold
    );
    tracing::debug!("requesting {}", url);

    let res = client.get(&url).send().await.context("request failed")?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        anyhow::bail!("{}: {}", status, text);
    }

    let hits: Vec<Hit> = res.json().await.context("invalid response body")?;

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
