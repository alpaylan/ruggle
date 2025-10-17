# Roogle
Roogle is a Rust API search engine, which allows you to search functions by names and type signatures.

## Progress

### Available Queries
- [x] Function queries
- [x] Method queries

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

## Example
```sh
$ cargo r --release
# Then, open http://localhost:8000/ in your browser for the web UI
# Or use the API directly:
$ curl -G \
      --data-urlencode "query=fn (Option<Result<T, E>>) -> Result<Option<T>, E>>" \
      --data-urlencode "scope=set:libstd" \
      --data-urlencode "limit=30" \
      --data-urlencode "threshold=0.4" \
      "http://localhost:8000/search"
```

## Example with Docker
```sh
$ docker-compose up
# Then, open http://localhost:8000/ in your browser for the web UI
# Or use the API directly:
$ curl -G \
      --data-urlencode "query=fn (Option<Result<T, E>>) -> Result<Option<T>, E>>" \
      --data-urlencode "scope=set:libstd" \
      --data-urlencode "limit=30" \
      --data-urlencode "threshold=0.4" \
      "http://localhost:8000/search"
```

## Query Syntax

- `fn f(type) -> type`
- `fn (type) -> type`
- `fn(type) -> type`
- `(type) -> type`

## Related Project
- [cargo-roogle](https://github.com/roogle-rs/cargo-roogle)

## Web UI

After starting the server, visit `http://localhost:8000/` for a simple search UI. It lets you:
- Choose a `scope` from `/scopes`
- Enter a `query`
- Adjust `limit` and `threshold`
- See results with names, breadcrumb paths, and convenient doc links

## CLI Client

Build and run the CLI (server must be running):

```sh
$ cargo run -p roogle --bin roogle-cli -- --scope set:libstd -- "fn (Option<Result<T, E>>) -> Result<Option<T>, E>>"
```

Flags: `--host` (default `http://localhost:8000`), `--scope`, `--limit`, `--threshold`, `--json`.

## VSCode Extension (local)

The extension lives in `vscode-roogle/`.

1. `cd vscode-roogle && npm install && npm run compile`
2. In VSCode, run "Extensions: Install from VSIX" (or launch with F5 in the extension host)
3. Command Palette: "Roogle: Search APIs"

Settings: `roogle.host`, `roogle.scope`, `roogle.limit`, `roogle.threshold`.
