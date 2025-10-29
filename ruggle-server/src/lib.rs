use std::{
    collections::{HashMap, HashSet},
    env::temp_dir,
    io::BufReader,
    path::Path,
};

use anyhow::{Context, Result};
use crates_io_api::AsyncClient;
use guppy::{graph::PackageGraph, MetadataCommand};
use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use ruggle_engine::{
    build_parent_index,
    query::parse::parse_query,
    search::{Hit, Scope, Set},
    types::{self, Crate, CrateMetadata},
    Index, Parent,
};
use ruggle_util::shake;

use serde::Deserialize as _;
use std::io::Read;
use tokio::{fs::OpenOptions, process::Command};
use tokio::{
    fs::{self},
    io::copy,
};
use tracing::{debug, error, info, warn};

pub fn perform_search(
    index: &Index,
    scopes: &Scopes,
    query_str: &str,
    scope_str: &str,
    limit: Option<usize>,
    threshold: Option<f32>,
) -> anyhow::Result<Vec<Hit>> {
    tracing::info!(
        "performing search for query `{}` in scope `{}`",
        query_str,
        scope_str
    );

    tracing::debug!("available scopes: {:?}", scopes.sets.keys());
    tracing::debug!("available crates: {:?}", scopes.krates);
    let scope =
        Scope::try_from(scope_str).context(format!("parsing scope `{}` failed", scope_str))?;
    debug!(?scope);

    let query = parse_query(query_str)
        .ok()
        .context(format!("parsing query `{}` failed", query_str))?
        .1;
    debug!(?query);

    let limit = limit.unwrap_or(30);
    let threshold = threshold.unwrap_or(0.4);
    let krates = scopes.get(&scope)?;

    let hits = index
        .search(&query, &krates, threshold)
        .with_context(|| format!("search with query `{:?}` failed", query))?;
    let hits = hits
        .into_iter()
        .inspect(|hit| debug!(?hit.name, link = ?hit.link, similarities = ?hit.similarities(), score = ?hit.similarities().score()))
        .take(limit)
        .collect::<Vec<_>>();

    Ok(hits)
}

pub async fn make_index(index_dir: &Path) -> Result<Index> {
    let crate_dir = index_dir.join("crate");
    info!("building index from {}", crate_dir.display());

    // Gather file list, preferring .zst over .json
    let mut entries = vec![];
    let mut dir = fs::read_dir(&crate_dir)
        .await
        .context("failed to read index files")?;
    while let Some(entry) = dir
        .next_entry()
        .await
        .context("failed to read index files")?
    {
        let path = entry.path();
        // Skip all raw .json if a .bin version exists
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let bin_path = path.with_extension("bin");
            if bin_path.exists() {
                continue;
            }
        }
        // Only include .json or .bun files
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext == "json" || ext == "bin" {
                entries.push(path);
            }
        }
    }

    info!("found {} crate files", entries.len());

    let t_start = std::time::Instant::now();

    // Parallel deserialization of all crates
    let crates: HashMap<CrateMetadata, _> = entries
        .par_iter()
        .filter_map(|path| {
            // Skip `<krate_name>.parents.bin` files
            if path
                .file_name()
                .and_then(|f| f.to_str())
                .map(|f| f.ends_with(".parents.bin"))
                .unwrap_or(false)
            {
                return None;
            }
            let file = std::fs::File::open(path).ok()?;
            let mut reader = BufReader::new(file);

            let ext = path.extension().and_then(|e| e.to_str());

            let t0 = std::time::Instant::now();
            let krate: Result<Crate> = match ext {
                Some("bin") => {
                    bincode::decode_from_reader(&mut reader, bincode::config::standard())
                        .with_context(|| format!("Failed to bincode::decode {}", path.display()))
                }
                _ => serde_json::from_reader(&mut reader)
                    .map_err(|e| {
                        eprintln!(
                            "error while serde_json::from_reader({}) => {e:?}",
                            path.display()
                        );
                        e
                    })
                    .with_context(|| {
                        format!("Failed to serde_json::from_reader {}", path.display())
                    }),
            };
            if let Err(ref e) = krate {
                warn!("deserializing {:?} failed: {}", path.display(), e);
                return None;
            }
            let mut krate = krate.unwrap();
            let krate_name: String = path.file_stem()?.to_str()?.to_owned();
            krate.name = Some(krate_name.clone());

            debug!("deserialized {:?} in {:?}", path.display(), t0.elapsed());
            let krate_metadata = CrateMetadata {
                name: krate_name,
                version: krate.crate_version.clone(),
            };
            // Rust 1.90 does not support `Path::file_prefix`, use `file_stem` instead
            Some((krate_metadata, krate))
        })
        .collect();

    let parents: HashMap<CrateMetadata, HashMap<types::Id, Parent>> = crates
        .par_iter()
        .map(|(krate_name, krate)| {
            // If `<krate_name>.parents.bin` exists, load it instead of building from scratch
            let parents_path = crate_dir.join(format!("{}.parents.bin", krate_name));
            if parents_path.exists() {
                let file = std::fs::File::open(&parents_path)
                    .expect("parents index file existence was already checked");
                let mut reader = BufReader::new(file);
                let parent_map: HashMap<types::Id, Parent> =
                    bincode::decode_from_reader(&mut reader, bincode::config::standard())
                        .expect("decoding parents index from bin failed");
                return (krate_name.clone(), parent_map);
            }
            // Otherwise, build parents index from scratch
            let parent_map = build_parent_index(krate);
            // Serialize parents index to `<krate_name>.parents.bin` for future use
            let mut file =
                std::fs::File::create(&parents_path).expect("creating parents index file failed");
            bincode::encode_into_std_write(&parent_map, &mut file, bincode::config::standard())
                .expect("encoding parents index to bin failed");
            tracing::debug!("serialized parents index to {:?}", parents_path);
            (krate_name.clone(), parent_map)
        })
        .collect();

    let total_time = t_start.elapsed();
    info!(
        "loaded {} crates in {:.2?} (avg {:.1?} each)",
        crates.len(),
        total_time,
        total_time / (crates.len().max(1) as u32)
    );

    Ok(Index { crates, parents })
}

fn dir_size(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| std::fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0))
        .sum()
}

pub fn shake_index(index_dir: &Path) -> Result<()> {
    // Measure index size before shaking
    let before = dir_size(&index_dir.join("crate"));
    let result = std::fs::read_dir(format!("{}/crate", index_dir.display()))
        .context("failed to read index files")?
        .map(|entry| {
            let entry = entry?;
            let json = std::fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read `{:?}`", entry.file_name()))?;
            let mut deserializer = serde_json::Deserializer::from_str(&json);
            deserializer.disable_recursion_limit();
            let krate = rustdoc_types::Crate::deserialize(&mut deserializer)
                .with_context(|| format!("failed to deserialize `{:?}`", entry.file_name()))?;
            let file_name = entry
                .path()
                .with_extension("")
                .file_name()
                .with_context(|| format!("failed to get file name from `{:?}`", entry.path()))?
                .to_str()
                .context("failed to get `&str` from `&OsStr`")?
                .to_owned();
            let krate = shake(krate);

            let json = serde_json::to_string(&krate)
                .with_context(|| format!("failed to serialize crate `{}`", &file_name))?;
            std::fs::write(
                format!("{}/crate/{}.json", index_dir.display(), file_name),
                json,
            )
            .with_context(|| format!("failed to write crate `{}`", &file_name))?;

            Ok(())
        })
        .collect::<Result<Vec<()>>>();
    // Measure index size after shaking
    let after = dir_size(&index_dir.join("crate"));
    tracing::info!(
        "index shaken: {:.2} MB → {:.2} MB (−{:.2} MB, {:.1}% smaller)",
        before as f64 / 1_048_576.0,
        after as f64 / 1_048_576.0,
        (before - after) as f64 / 1_048_576.0,
        (before - after) as f64 / before as f64 * 100.0
    );

    result.map(|_| ())
}

pub fn generate_bin_index(index_dir: &Path) -> Result<()> {
    let _result = std::fs::read_dir(format!("{}/crate", index_dir.display()))
        .context("failed to read index files")?
        .map(|entry| {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("bin") {
                // Skip already generated bin files
                tracing::debug!(
                    "skipping already generated bin file {:?}",
                    entry.file_name()
                );
                return Ok(());
            }
            let json = std::fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read `{:?}`", entry.file_name()))?;
            let mut deserializer = serde_json::Deserializer::from_str(&json);
            deserializer.disable_recursion_limit();
            tracing::debug!("generating bin for {:?}", entry.file_name());

            let krate = Crate::deserialize(&mut deserializer);

            let Ok(krate) = krate else {
                warn!(
                    "deserializing {:?} failed: {}",
                    entry.file_name(),
                    krate.unwrap_err()
                );
                return Ok(());
            };

            let file_name = entry
                .path()
                .with_extension("")
                .file_name()
                .with_context(|| format!("failed to get file name from `{:?}`", entry.path()))?
                .to_str()
                .context("failed to get `&str` from `&OsStr`")?
                .to_owned();

            let mut file = std::fs::File::create(format!(
                "{}/crate/{}.bin",
                index_dir.display(),
                file_name
            ))
            .with_context(|| format!("failed to create bin file for crate `{}`", &file_name))?;
            bincode::encode_into_std_write(&krate, &mut file, bincode::config::standard())
                .with_context(|| format!("failed to serialize crate `{}` to bin", &file_name))?;

            Ok(())
        })
        .collect::<Result<Vec<()>>>();

    Ok(())
}

pub struct Scopes {
    pub sets: HashMap<String, Set>,
    pub krates: HashSet<CrateMetadata>,
}

impl Scopes {
    pub fn get(&self, scope: &Scope) -> Result<Vec<CrateMetadata>> {
        match scope {
            Scope::Set(set) => self
                .sets
                .get(set)
                .map(|s| s.crates.clone())
                .with_context(|| format!("set `{}` not found", set)),
            Scope::Crate(krate_metadata) => self
                .krates
                .get(krate_metadata)
                .map(|s| vec![s.clone()])
                .with_context(|| format!("crate `{}` not found", krate_metadata)),
        }
    }
}

pub fn make_sets(index_dir: &Path) -> HashMap<String, Set> {
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
                    let set = path.file_stem().unwrap().to_str().unwrap().to_owned(); // SAFETY: files in `ruggle-index` has a name.
                    let krates = serde_json::from_str::<Vec<CrateMetadata>>(&json)
                        .context(format!("failed to deserialize set `{}`", &set))?;

                    Ok((
                        set.clone(),
                        Set {
                            name: set,
                            crates: krates,
                        },
                    ))
                })
                .filter_map(|res: Result<_, anyhow::Error>| {
                    if let Err(ref e) = res {
                        warn!("registering a scope skipped: {}", e)
                    }
                    res.ok()
                })
                .collect()
        }
    }
}

pub async fn pull_crate_from_docs_rs(metadata: &types::CrateMetadata) -> Result<types::Crate> {
    info!("checking docs.rs for crate: {}", &metadata.name);
    let url = format!(
        "https://docs.rs/crate/{}/{}/json",
        metadata.name, metadata.version
    );
    debug!("docs.rs url for {}: {}", metadata.name, url);

    let client = reqwest::Client::new();
    let response = client.get(&url).send().await?;
    debug!("response status: {}", response.status());
    if response.status().is_success() {
        debug!("docs.rs url for {}: {}", metadata.name, url);
        debug!("response: {:?}", response);
        let zst_encoded_krate = response.bytes().await?;
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&zst_encoded_krate[..]).unwrap();
        let mut json_encoded_krate = Vec::new();
        decoder
            .read_to_end(&mut json_encoded_krate)
            .with_context(|| format!("Failed to create zstd decoder for {}", url))?;

        let mut krate: types::Crate = serde_json::from_slice(&json_encoded_krate)
            .with_context(|| format!("Failed to serde_json::from_slice {}", url))?;
        krate.name = Some(metadata.name.clone());
        info!("fetched crate {} from docs.rs", metadata);
        return Ok(krate);
    }

    Err(anyhow::anyhow!("crate {} not found on docs.rs", metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pull_crate_from_docs_rs() {
        tracing_subscriber::fmt::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .init();
        let krate = types::CrateMetadata {
            name: "serde".into(),
            version: "latest".into(),
        };
        let result = pull_crate_from_docs_rs(&krate).await;
        assert!(result.is_ok());
    }
}

pub async fn pull_crate_from_remote_index(
    krate_metadata: &types::CrateMetadata,
) -> Result<types::Crate> {
    info!("checking remote index for crate: {}", &krate_metadata.name);
    let bin_url = format!(
        "https://raw.githubusercontent.com/alpaylan/ruggle-index/main/crate/{}.bin",
        krate_metadata.name
    );
    let json_url = format!(
        "https://raw.githubusercontent.com/alpaylan/ruggle-index/main/crate/{}.json",
        // "https://docs.rs/crate/{}/{}/json",
        krate_metadata.name,
        // krate_metadata.version // FIXME: Version-specific crates are not supported in the remote index yet
    );

    let client = reqwest::Client::new();

    // Try to fetch .bin first
    debug!(".bin url for {}: {}", krate_metadata, bin_url);
    let response = client.get(&bin_url).send().await?;
    if response.status().is_success() {
        let bytes = response.bytes().await?;
        if let Ok((krate, _)) =
            bincode::decode_from_slice::<types::Crate, _>(&bytes, bincode::config::standard())
        {
            info!("fetched crate {} from remote index (.bin)", krate_metadata);
            return Ok(krate);
        }
    }
    tracing::debug!(
        "crate {} not found in remote index (.bin), trying .json",
        krate_metadata
    );

    // Fallback to .json
    debug!(".json url for {}: {}", krate_metadata, json_url);
    let response = client.get(&json_url).send().await?;
    if response.status().is_success() {
        println!("response: {:?}", response);
        // If it's a
        let text = response.text().await?;
        let mut krate: types::Crate = serde_json::from_str(&text)
            .with_context(|| format!("Failed to serde_json::from_str {}", json_url))?;
        krate.name = Some(krate_metadata.name.clone());
        info!(
            "fetched crate {} from remote index (.json)",
            krate_metadata.name
        );
        return Ok(krate);
    }

    Err(anyhow::anyhow!(
        "crate {} not found in remote index",
        krate_metadata
    ))
}

pub async fn pull_set_from_remote_index(set_name: &str) -> Result<Vec<CrateMetadata>> {
    info!("fetching set {} from remote index", set_name);
    let json_url = format!(
        "https://raw.githubusercontent.com/alpaylan/ruggle-index/main/set/{}.json",
        set_name
    );

    let client = reqwest::Client::new();
    let response = client.get(&json_url).send().await?;
    if response.status().is_success() {
        let text = response.text().await?;
        let krates: Vec<CrateMetadata> = serde_json::from_str(&text)
            .with_context(|| format!("Failed to serde_json::from_str {}", json_url))?;
        info!("fetched set {} from remote index", set_name);
        return Ok(krates);
    }

    Err(anyhow::anyhow!(
        "set {} not found in remote index",
        set_name
    ))
}

async fn index_krate(krate: &crates_io_api::Crate) -> Result<types::Crate> {
    let temp = temp_dir();
    let path = temp.join(format!("{}.tar.gz", krate.name));
    let url = format!(
        "https://static.crates.io/crates/{name}/{name}-{version}.crate",
        name = krate.name,
        version = krate.max_version,
    );

    let resp = reqwest::get(url).await?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await
        .context("Could not create the temp tar.gz file")?;

    copy(&mut resp.bytes().await?.as_ref(), &mut file)
        .await
        .context("tokio::io::copy failed")?;

    Command::new("tar")
        .args(["-xf", &format!("{}.tar.gz", krate.name)])
        .current_dir(&temp)
        .status()
        .await
        .context("Failed to extract tar.gz file")?;

    let unpacked = temp.join(format!("{}-{}", krate.name, krate.max_version));
    let cargo = Command::new("cargo")
        .args(["+nightly", "rustdoc"])
        .env("RUSTDOCFLAGS", "--output-format=json -Z unstable-options")
        .current_dir(&unpacked)
        .status()
        .await
        .context("Failed to run cargo rustdoc")?;
    if !cargo.success() {
        return Err(anyhow::anyhow!(
            "cargo rustdoc failed for crate {}",
            krate.name
        ));
    }
    // check the `target/doc` contents
    let doc_dir = unpacked.join("target/doc");
    if !doc_dir.exists() {
        return Err(anyhow::anyhow!(
            "doc directory does not exist for crate {}",
            krate.name
        ));
    }
    let mut doc_dir_reader = fs::read_dir(&doc_dir).await?;
    let krate_file_path = loop {
        if let Some(entry) = doc_dir_reader
            .next_entry()
            .await
            .context("Failed to read doc directory")?
        {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.ends_with(".json") {
                break entry.path();
            }
        } else {
            return Err(anyhow::anyhow!(
                "No JSON file found in doc directory for crate {}",
                krate.name
            ));
        }
    };
    let mut krate_: types::Crate = serde_json::from_slice(
        &fs::read(&krate_file_path)
            .await
            .context("Failed to read crate JSON file")?,
    )
    .with_context(|| format!("Failed to serde_json::from_slice for crate {}", krate.name))?;

    krate_.name = Some(krate.name.clone());

    info!("built crate {} locally", krate.name);

    Ok(krate_)
}

pub async fn build_crate_locally(metadata: &types::CrateMetadata) -> Result<types::Crate> {
    let client = AsyncClient::new(
        "ruggle (akeles@umd.edu)",
        std::time::Duration::from_millis(1000),
    )?;

    let krate = client
        .get_crate(&metadata.name)
        .await
        .context(format!("failed to get crate info: {}", &metadata.name))?
        .crate_data;

    index_krate(&krate).await
}

pub async fn index_local_crate(
    index: &mut Index,
    cargo_manifest_path: &Path,
) -> Result<Vec<types::Crate>> {
    let krates_metadata = gather_all_dependencies(cargo_manifest_path)
        .context("failed to gather all transitive dependencies")?;

    tracing::info!(
        "gathered {} dependencies from Cargo.toml",
        krates_metadata.len()
    );
    tracing::debug!("dependencies: {:?}", krates_metadata);

    let mut krates: Vec<types::Crate> = Vec::new();
    for krate_metadata in &krates_metadata {
        if let Some(krate) = index.crates.get(krate_metadata).cloned() {
            info!("crate is already indexed: {}", &krate_metadata);
            krates.push(krate);
        } else if let Ok(krate) = pull_crate_from_remote_index(krate_metadata).await {
            krates.push(krate);
        // FIXME: docs.rs is unreliable sometimes, and we also need to differentiate crates that have a different local version
        // } else if let Ok(krate) = pull_crate_from_docs_rs(krate_metadata).await {
        //     krates.push(krate);
        } else if let Ok(krate) = build_crate_locally(krate_metadata).await {
            krates.push(krate);
        } else {
            error!("failed to index crate: {}", &krate_metadata);
        }
    }

    Ok(krates)
}

#[cfg(test)]
mod dependency_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_gather_all_dependencies() {
        let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("ruggle-engine")
            .join("Cargo.toml");
        let deps = gather_all_dependencies(&manifest_path).unwrap();
        println!("dependencies: {:#?}", deps);
        assert!(deps.iter().any(|d| d.name == "ruggle-util"));
    }
}

pub fn gather_all_dependencies(cargo_manifest_path: &Path) -> anyhow::Result<Vec<CrateMetadata>> {
    let metadata = MetadataCommand::new()
        .manifest_path(cargo_manifest_path)
        .exec()?;

    let graph = PackageGraph::from_metadata(metadata)?;
    let mut packages = Vec::new();

    for member in graph.workspace().iter() {
        for link in member.direct_links() {
            let pkg = link.to();
            packages.push(CrateMetadata {
                name: pkg.name().to_string(),
                version: pkg.version().to_string(),
            });
        }
    }
    Ok(packages)
}

pub fn gather_all_transitive_dependencies(
    cargo_manifest_path: &Path,
) -> anyhow::Result<Vec<CrateMetadata>> {
    let metadata = MetadataCommand::new()
        .manifest_path(cargo_manifest_path)
        .exec()?;
    let graph = PackageGraph::from_metadata(metadata)?;
    let packages = graph
        .packages()
        .map(|pkg| CrateMetadata {
            name: pkg.name().to_string(),
            version: pkg.version().to_string(),
        })
        .collect();
    Ok(packages)
}
