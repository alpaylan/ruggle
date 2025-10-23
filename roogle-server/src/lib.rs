use std::{
    collections::HashMap,
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use roogle_engine::{search::Scope, types::Crate, Index};
use roogle_util::shake;

use serde::Deserialize as _;
use tracing::{debug, info, warn};

pub fn make_index(index_dir: &Path) -> Result<Index> {
    let crate_dir = index_dir.join("crate");
    info!("building index from {}", crate_dir.display());

    // Gather file list, preferring .zst over .json
    let mut entries = vec![];
    for entry in fs::read_dir(&crate_dir).context("failed to read index files")? {
        let path = entry?.path();
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
    let crates: HashMap<String, _> = entries
        .par_iter()
        .filter_map(|path| {
            let file = File::open(&path).ok()?;
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
            krate.name = path.file_stem()?.to_str()?.to_owned().into();
            debug!("deserialized {:?} in {:?}", path.display(), t0.elapsed());

            Some((path.file_prefix()?.to_str()?.to_owned(), krate))
        })
        .collect();

    let total_time = t_start.elapsed();
    info!(
        "loaded {} crates in {:.2?} (avg {:.1?} each)",
        crates.len(),
        total_time,
        total_time / (crates.len().max(1) as u32)
    );

    Ok(Index { crates })
}

fn dir_size(path: &std::path::Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| fs::metadata(e.path()).map(|m| m.len()).unwrap_or(0))
        .sum()
}

pub fn shake_index(index_dir: &PathBuf) -> Result<()> {
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

pub fn generate_bin_index(index_dir: &PathBuf) -> Result<()> {
    let result = std::fs::read_dir(format!("{}/crate", index_dir.display()))
        .context("failed to read index files")?
        .map(|entry| {
            let entry = entry?;
            if entry.path().extension().and_then(|e| e.to_str()) == Some("bin") {
                // Skip already generated bin files
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

            let mut file = File::create(format!("{}/crate/{}.bin", index_dir.display(), file_name))
                .with_context(|| format!("failed to create bin file for crate `{}`", &file_name))?;
            bincode::encode_into_std_write(&krate, &mut file, bincode::config::standard())
                .with_context(|| format!("failed to serialize crate `{}` to bin", &file_name))?;

            Ok(())
        })
        .collect::<Result<Vec<()>>>();

    Ok(())
}

pub struct Scopes {
    pub sets: HashMap<String, Scope>,
    pub krates: HashMap<String, Scope>,
}

pub fn make_scopes(index_dir: &Path) -> Result<Scopes> {
    tracing::info!("building scopes from {}", index_dir.display());
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
