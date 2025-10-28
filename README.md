# Ruggle

Ruggle is a fork of [Roogle](https://github.com/roogle-rs/roogle), a Rust API search engine that allow for searching functions using type signatures over Rust codebases.

The idea of API search for programming languages isn't novel, [Hoogle](https://wiki.haskell.org/index.php?title=Hoogle) has been around for more than 2 decades, Roogle itself is 4 years old,
OCaml has [Sherlodoc](https://github.com/art-w/sherlodoc), Lean recently announced [Loogle!](https://loogle.lean-lang.org)...

The vision for Ruggle is not to just to be yet another API search tool, but become the default mode of search when working on a Rust project, making structural search a first class citizen
developer tooling. When searching in a codebase, it is rare that we are without context; however text search forces us to build a textual context when we already have access to much more structural
information. The objective of Ruggle is to build this graph of structured information, and allow the user to query it as a better search engine.

Roogle seems to be in public archival mode, that's why I've decided to publish Ruggle as a separate project. There are already some major changes to the project:

1. Ruggle is available as a VSCode extension that automatically downloads and starts the Ruggle server
2. Ruggle maintains its own vendored `rustdoc_types` for faster decoding using `bincode`
3. Ruggle allows for automatically indexing and searching through the current Rust project

These are some initial steps for the project, to be followed by better and faster search, better autocomplete in the search bar,
a large index of available projects on [crates.io](https://crates.io), code actions for filling type holes in incomplete Rust programs,
as well as more complex queries.

## Installation

Installation with VSCode is available in the extensions bar and the marketplace as well as through the CLI:

```bash
code --install-extension AlperenKeles.ruggle
```

The first time you run the extension, it will automatically download the server from the latest release.
Alternatively, you can download latest release of the server via:

```bash
curl -fsSL https://raw.githubusercontent.com/alpaylan/ruggle/main/install.sh | bash
```

The server is also available in crates.io:

```bash
cargo install ruggle-server
```

Once you have the server, you can locally run it via:

```bash
ruggle-server --host 127.0.0.1 --port 8000
```

By default the server looks for the index in `$HOME/.ruggle` which you can override with `--index <path>`.

## Roadmap

### Available Queries

- [x] Function queries: `fn <name>(<arg-name>: <type>, <arg-name>: <type>) -> <type>`
- [ ] Multi-hop function queries: `(A -> ... -> C)`
- [ ] Scoped queries: `<mod|struct|enum> <symbol>: <funtion-query>`,
- [ ] Macro queries

### Available Types to Query

- [x] Primitive types
- [ ] Generic types
  - [x] Without bounds and where predicates (e.g., `<T>`)
  - [ ] With bounds (e.g., `<T: Copy>`)
  - [ ] With where predicates
- [x] Custom types
  - [x] Without generic args (e.g., `IpAddr`)
  - [x] With generic args (e.g., `Vec<T>`, `Option<T>`)
- [ ] Other types

### Realtime Search

- [ ] Rust analyzer integration for realtime updates to the index for the current Rust project
- [ ] Faster search with better search indexes, currently all search is O(N)
- [ ] Better search heuristics, deprioritizing functions that are too loosely typed
- [ ] Better search UX, autocomplete existing symbols
- [ ] Code actions for filling typed holes, autocomplete suggestions based on the expected type vs the type of the cursor term

## Web UI

After starting the server, visit `http://localhost:8000/` for a simple search UI. It lets you:

- Choose a `scope` from `/scopes`
- Enter a `query`
- Adjust `limit` and `threshold`
- See results with names, breadcrumb paths, convenient doc links

## CLI Client

Build and run the CLI, you can both run an standalone search and request to an existing server:

```sh
$ cargo run --bin ruggle-cli -- --scope set:libstd --query "fn (Option<Result<T, E>>) -> Result<Option<T>, E>>"
```

When asking the server, you set `--host` for the server URL.

```sh
$ cargo run --bin ruggle-cli -- --host "http://127.0.0.1:58034" --scope crate:tracing:0.1.41 --query "fn (Option<Result<T, E>>) -> Result<Option<T>, E>>"
```

Flags: `--host` (default `http://localhost:8000`), `--scope`, `--limit`, `--threshold`, `--json`.

## VSCode Extension (local)

The extension lives in `vscode-ruggle/`.

1. `cd vscode-ruggle && npm package package &&  code --install-extension ./ruggle-0.0.2.vsix`
2. Command Palette: "ruggle: Search APIs"

There are a variety of settings including: `ruggle.host`, `ruggle.scope`, `ruggle.limit`, `ruggle.threshold`, `ruggle.server`.
