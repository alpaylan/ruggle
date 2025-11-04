use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ruggle_engine::types::CrateMetadata;
use ruggle_server::{make_index, perform_search, Scopes};
use tracing::Level;

fn workspace_path(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for part in parts {
        p = p.join(part);
    }
    p
}

fn run_rustdoc_json(crate_dir: &Path) {
    // Requires nightly toolchain available in environment.
    let status = Command::new("cargo")
        .arg("+nightly")
        .arg("rustdoc")
        .env("RUSTDOCFLAGS", "--output-format=json -Z unstable-options")
        .current_dir(crate_dir)
        .status()
        .expect("failed to run cargo rustdoc");
    assert!(status.success(), "cargo rustdoc did not succeed");
}

fn find_crate_json(crate_dir: &Path, crate_name: &str) -> PathBuf {
    let doc_dir = crate_dir.join("target/doc");
    let json = doc_dir.join(format!("{}.json", crate_name));
    assert!(json.exists(), "expected rustdoc json at {:?}", json);
    json
}

#[tokio::test]
async fn index_local_test_crate_and_query() {
    // Initialize logging for debugging if needed
    let _ = tracing_subscriber::fmt::fmt()
        .with_max_level(Level::TRACE)
        .with_file(true)
        .with_line_number(true)
        .without_time()
        .with_test_writer()
        .try_init();

    // 1) Build rustdoc JSON for the local `test` crate
    let workspace_root = workspace_path(&[".."]);
    let test_crate_dir = workspace_path(&["..", "test"]);
    tracing::info!(
        "building rustdoc json for test crate at {}",
        test_crate_dir.display()
    );
    run_rustdoc_json(&test_crate_dir);

    // 2) Prepare a temporary index directory layout: <tmp>/crate/test.json
    let tmp_root =
        std::env::temp_dir().join(format!("ruggle_server_index_test_{}", std::process::id()));
    let crate_dir = tmp_root.join("crate");
    fs::create_dir_all(&crate_dir).expect("failed to create temp index dir");
    let src_json = find_crate_json(&workspace_root, "test");
    let dst_json = crate_dir.join("test.json");
    fs::copy(&src_json, &dst_json).expect("failed to copy rustdoc json into index dir");

    // 3) Build an Index from the temp directory
    let index = make_index(&tmp_root).await.expect("make_index failed");

    // 4) Build Scopes for the crate `test`
    let mut scopes = Scopes {
        sets: HashMap::new(),
        krates: HashSet::new(),
    };
    let test_meta: CrateMetadata = index
        .crates
        .keys()
        .find(|m| m.name == "test")
        .expect("test crate not found in index")
        .clone();
    scopes.krates.insert(test_meta.clone());

    // list all items in the index
    for krate in index.crates.values() {
        tracing::info!("krate: {}", krate);
    }

    // 5) Run a simple query that should match a known function in `test`
    // e.g., `util::text::split_words`
    let scope_str = format!("crate:{}:{}", test_meta.name, test_meta.version);
    let hits = perform_search(
        &index,
        &scopes,
        "fn split_words(&str) -> Vec<String>",
        &scope_str,
        Some(20),
        Some(0.4),
    )
    .expect("search failed");

    tracing::info!("hits: {:?}", hits);

    assert!(
        hits.iter().any(|h| h.name == "split_words"),
        "expected to find split_words, got: {:?}",
        hits.iter().map(|h| h.name.clone()).collect::<Vec<_>>()
    );
}
