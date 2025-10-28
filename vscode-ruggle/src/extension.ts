import * as vscode from 'vscode';
import * as https from 'node:https';
import * as http from 'node:http';
import { spawn } from 'node:child_process';
import type { ChildProcess } from 'node:child_process';
import * as os from 'node:os';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as crypto from 'node:crypto';

async function fetchJson<T>(url: string, signal?: AbortSignal): Promise<T> {
    // Prefer VS Code's global fetch when available
    if (typeof fetch !== 'undefined') {
        const res = await fetch(url, { signal });
        if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
        return res.json();
    }
    // Fallback to Node http/https
    const client = url.startsWith('https') ? https : http;
    return new Promise<T>((resolve, reject) => {
        const req = client.get(url, (res) => {
            const status = res.statusCode ?? 0;
            if (status < 200 || status >= 300) {
                reject(new Error(`${status}`));
                return;
            }
            const chunks: Buffer[] = [];
            res.on('data', (c) => chunks.push(c));
            res.on('end', () => {
                try {
                    const text = Buffer.concat(chunks).toString('utf8');
                    resolve(JSON.parse(text));
                } catch (e) {
                    reject(e);
                }
            });
        });
        req.on('error', reject);
        if (signal) signal.addEventListener('abort', () => req.destroy(new Error('AbortError')));
        req.end();
    });
}

let outChan: vscode.OutputChannel | undefined;
let serverProc: ChildProcess | undefined;

type Scope =
    | { "Crate": string }
    | { "Set": [string, string[]] }

// Types mirrored from the server's JSON schema
interface Hit {
    id: number;
    name: string;
    path: string[];
    link: string;
    docs?: string | null;
    signature: string;
}

type CrateMetadata = {
    name: string;
    version: string;
};

function errorMessage(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}

export function activate(context: vscode.ExtensionContext) {
    outChan = vscode.window.createOutputChannel('Ruggle');
    const searchCmd = vscode.commands.registerCommand('ruggle.search', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        // Ensure server is reachable or start it if configured
        const ok = await ensureServer(cfg);
        if (!ok) {
            vscode.window.showErrorMessage('Ruggle server is not reachable. Configure ruggle.host or enable autoStart.');
            return;
        }
        const host: string = cfg.get('host', 'http://localhost:8000');
        const scope: string = cfg.get('scope', 'set:libstd');
        const limit: number = cfg.get('limit', 30);
        const threshold: number = cfg.get('threshold', 0.4);
        outChan?.appendLine(`[Ruggle] Opening QuickPick (dynamic search) host=${host} scope=${scope}`);
        await presentSearch(host, scope, '', limit, threshold);
    });

    const searchSelCmd = vscode.commands.registerCommand('ruggle.searchSelection', async () => {
        const editor = vscode.window.activeTextEditor;
        const selected = editor?.document.getText(editor.selection).trim();
        if (!selected) {
            vscode.window.showInformationMessage('No selection to search');
            return;
        }
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        const scope: string = cfg.get('scope', 'set:libstd');
        const limit: number = cfg.get('limit', 30);
        const threshold: number = cfg.get('threshold', 0.4);
        await presentSearch(host, scope, selected, limit, threshold);
    });

    const setHostCmd = vscode.commands.registerCommand('ruggle.setHost', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const current: string = cfg.get('host', 'http://localhost:8000');
        const input = await vscode.window.showInputBox({
            prompt: 'Ruggle server host URL',
            value: current,
        });
        if (input) {
            await cfg.update('host', input, vscode.ConfigurationTarget.Global);
            vscode.window.showInformationMessage(`Ruggle host set to ${input}`);
        }
    });

    const setScopeCmd = vscode.commands.registerCommand('ruggle.setScope', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const current: string = cfg.get('scope', 'set:libstd');
        try {
            const scopes: string[] = await withPortRecovery(cfg, async (h) => fetchJson(`${h}/scopes`));
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Ruggle: Set Scope',
                placeHolder: current,
            });
            if (picked) {
                await cfg.update('scope', picked, vscode.ConfigurationTarget.Global);
                vscode.window.showInformationMessage(`Ruggle scope set to ${picked}`);
            }
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Ruggle error: ${errorMessage(e)}`);
        }
    });

    const startServerCmd = vscode.commands.registerCommand('ruggle.startServer', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const ok = await ensureServer(cfg);
        vscode.window.showInformationMessage(ok ? 'Ruggle server is running' : 'Failed to start Ruggle server');
    });
    const stopServerCmd = vscode.commands.registerCommand('ruggle.stopServer', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        try {
            await withPortRecovery(cfg, async (h) => fetch(`${h}/stop`, { method: 'POST', signal: AbortSignal.timeout(1000) }));
            outChan?.appendLine('[Ruggle] Sent /stop');
        } catch (e: unknown) {
            outChan?.appendLine(`[Ruggle] /stop failed: ${errorMessage(e)}`);
        }
    });

    const updateIndexCmd = vscode.commands.registerCommand('ruggle.updateIndex', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const input = await vscode.window.showInputBox({
            prompt: 'Enter crate:<name> or set:<name> (e.g., crate:std or set:libstd)',
            placeHolder: 'crate:std | set:libstd'
        });
        if (!input) return;

        async function buildScope(): Promise<Scope> {
            if (!input) {
                throw new Error('No input provided');
            }
            const trimmed = input.trim();
            if (trimmed.startsWith('set:')) {
                const setName = trimmed.slice(4);
                return {
                    "Set": [setName, []]
                };
            }
            const crateName = trimmed.startsWith('crate:') ? trimmed.slice(6) : trimmed;
            return {
                "Crate": crateName
            };
        }

        try {
            const scope = await buildScope();
            const scopes = { scopes: [scope] };
            const doPost = async (h: string) => {
                const target = `${h}/index`;
                outChan?.appendLine(`[Ruggle] POST ${target} scopes=${JSON.stringify(scopes)}`);
                const res = await fetch(target, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(scopes),
                    signal: AbortSignal.timeout(30000),
                });
                const text = await res.text();
                if (!res.ok) throw new Error(text || res.statusText);
                return text;
            };
            const text = await withPortRecovery(cfg, doPost);
            vscode.window.showInformationMessage(`Index update: ${text}`);
            outChan?.appendLine(`[Ruggle] Index update response: ${text}`);
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Index update failed: ${errorMessage(e)}`);
            outChan?.appendLine(`[Ruggle] Index update failed: ${errorMessage(e)}`);
        }
    });

    const installLibstdCmd = vscode.commands.registerCommand('ruggle.installLibstdIndex', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        const base = 'https://raw.githubusercontent.com/alpaylan/ruggle-index/main/crate';
        const names = ['std', 'core', 'alloc'];
        const urls = names.flatMap(n => [`${base}/${n}.bin`, `${base}/${n}.json`]);
        try {
            const doPost = async (h: string) => {
                const res = await fetch(`${h}/index`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ urls }),
                    signal: AbortSignal.timeout(20000),
                });
                const text = await res.text();
                if (!res.ok) throw new Error(text || res.statusText);
                return text;
            };
            const text = await withPortRecovery(cfg, doPost);
            vscode.window.showInformationMessage(`Index update: ${text}`);
            outChan?.appendLine(`[Ruggle] Index install libstd response: ${text}`);
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Index install failed: ${errorMessage(e)}`);
            outChan?.appendLine(`[Ruggle] Index install failed: ${errorMessage(e)}`);
        }
    });

    const showLogsCmd = vscode.commands.registerCommand('ruggle.showLogs', async () => {
        outChan?.show(true);
        outChan?.appendLine('[Ruggle] Logs opened');
    });

    const indexCurrentProjectCmd = vscode.commands.registerCommand('ruggle.indexCurrentProject', async () => {
        const folders = vscode.workspace.workspaceFolders;
        if (!folders || folders.length === 0) {
            vscode.window.showInformationMessage('No workspace folder open');
            return;
        }
        const root = folders[0].uri.fsPath;
        const manifest = path.join(root, 'Cargo.toml');
        if (!fs.existsSync(manifest)) {
            vscode.window.showInformationMessage('No Cargo.toml found at the workspace root');
            return;
        }
        const cfg = vscode.workspace.getConfiguration('ruggle');
        try {
            const doPost = async (h: string) => {
                const target = `${h}/index/local`;
                outChan?.appendLine(`[Ruggle] POST ${target} cargo_manifest_path=${manifest}`);
                const res = await fetch(target, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ cargo_manifest_path: manifest }),
                    signal: AbortSignal.timeout(120000),
                });
                const text = await res.text();
                if (!res.ok) throw new Error(text || res.statusText);
                return text;
            };
            const text = await withPortRecovery(cfg, doPost);
            vscode.window.showInformationMessage(`Local index: ${text}`);
            outChan?.appendLine(`[Ruggle] Local index response: ${text}`);
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Local index failed: ${errorMessage(e)}`);
            outChan?.appendLine(`[Ruggle] Local index failed: ${errorMessage(e)}`);
        }
    });

    const listIndexedCmd = vscode.commands.registerCommand('ruggle.listIndexed', async () => {
        const cfg = vscode.workspace.getConfiguration('ruggle');
        try {
            const names: CrateMetadata[] = await withPortRecovery(cfg, async (h) => fetchJson(`${h}/index`));
            outChan?.appendLine(`[Ruggle] Indexed crates: ${names.map(n => n.name).join(', ')}`);
            const picked = await vscode.window.showQuickPick(names.map(n => n.name), { title: 'Indexed Crates' });
            if (picked) { vscode.env.clipboard.writeText(picked); }
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Failed to list indexed: ${errorMessage(e)}`);
        }
    });

    context.subscriptions.push(searchCmd, searchSelCmd, setHostCmd, setScopeCmd, startServerCmd, stopServerCmd, updateIndexCmd, installLibstdCmd, listIndexedCmd, showLogsCmd, indexCurrentProjectCmd);
}

export function deactivate() { }

async function presentSearch(host: string, initialScope: string, query: string, limit: number, threshold: number) {
    const qp = vscode.window.createQuickPick<vscode.QuickPickItem & { link?: string }>();
    qp.matchOnDescription = true;
    qp.matchOnDetail = true;
    qp.placeholder = 'Type to refine results…';
    qp.title = 'Ruggle Results';
    qp.value = query;
    let scope = initialScope;
    qp.buttons = [
        {
            tooltip: 'Change Scope',
            iconPath: new vscode.ThemeIcon('gear')
        }
    ];

    let handle: NodeJS.Timeout | undefined;
    let tokenSrc = new AbortController();

    async function run(q: string) {
        if (q.length < 2) { qp.items = []; return; }
        const cfg = vscode.workspace.getConfiguration('ruggle');
        let currentHost: string = cfg.get('host', host);
        outChan?.appendLine(`[Ruggle] run q='${q.slice(0, 120)}'… limit=${limit} threshold=${threshold} scope=${scope} host=${currentHost}`);
        tokenSrc.abort();
        tokenSrc = new AbortController();
        const params = new URLSearchParams();
        params.set('scope', scope);
        params.set('limit', String(limit));
        params.set('threshold', String(threshold));
        qp.busy = true;
        try {
            const getAttempt = async (h: string): Promise<Hit[]> => {
                const getUrl = `${h}/search?${params.toString()}&query=${encodeURIComponent(q)}`;
                outChan?.appendLine(`[Ruggle] GET ${getUrl}`);
                return fetchJson<Hit[]>(getUrl, tokenSrc.signal);
            };
            let hits: Hit[] | null = null;
            try {
                hits = await withPortRecovery(cfg, getAttempt);
                currentHost = cfg.get('host', currentHost);
            } catch (getErr: unknown) {
                // Retry with POST body if GET fails (e.g., due to proxies or URL limits)
                outChan?.appendLine(`[Ruggle] GET failed: ${errorMessage(getErr)}`);
                if (typeof fetch !== 'undefined') {
                    const postAttempt = async (h: string): Promise<Hit[]> => {
                        const postUrl = `${h}/search?${params.toString()}&query=${encodeURIComponent(q)}`;
                        outChan?.appendLine(`[Ruggle] POST fallback ${postUrl}`);
                        const res = await fetch(postUrl, {
                            method: 'POST',
                            body: q,
                            signal: tokenSrc.signal,
                            headers: { 'Content-Type': 'text/plain; charset=utf-8' },
                        });
                        if (!res.ok) throw new Error(`${res.status} ${res.statusText} ${await res.text()}`);
                        return res.json() as Promise<Hit[]>;
                    };
                    hits = await withPortRecovery(cfg, postAttempt);
                    currentHost = cfg.get('host', currentHost);
                } else {
                    throw getErr;
                }
            }
            const safeHits = hits ?? [];
            outChan?.appendLine(`[Ruggle] Hits: ${JSON.stringify(hits)}`);
            const mapped = safeHits.map((h: Hit) => ({
                label: h.signature || h.name || '',
                description: (h.path || []).join('::'),
                detail: '',
                link: h.link,
                alwaysShow: true as boolean,
            }));
            qp.items = mapped;
            if (mapped.length > 0) {
                qp.activeItems = [mapped[0]];
            }
            outChan?.appendLine(`[Ruggle] Results: ${safeHits.length}`);
            const preview = mapped
                .slice(0, 5)
                .map(i => `${i.label} (${i.description})`)
                .join(' | ');
            outChan?.appendLine(`[Ruggle] First items: ${preview}`);
        } catch (e: unknown) {
            const abortName = (e && typeof e === 'object' && 'name' in e) ? String((e as { name?: unknown }).name) : '';
            if (abortName !== 'AbortError') {
                vscode.window.showErrorMessage(`Ruggle error: ${errorMessage(e)}`);
                qp.items = [{ label: 'Error fetching results', description: String(e) }];
                outChan?.appendLine(`[Ruggle] Error: ${errorMessage(e)}`);
            }
        } finally {
            qp.busy = false;
        }
    }

    qp.onDidChangeValue((v) => {
        // Always show a hint item so the list isn't visually empty
        if (v.length < 2) {
            qp.items = [{ label: 'Type at least 2 characters…', description: '' }];
        }
        if (handle) clearTimeout(handle);
        handle = setTimeout(() => run(v), 300);
    });
    qp.onDidAccept(() => {
        const picked = qp.selectedItems[0] as { link?: string } | undefined;
        if (picked?.link) {
            outChan?.appendLine(`[Ruggle] Opening docs: ${picked.link}`);
            vscode.env.openExternal(vscode.Uri.parse(picked.link));
        } else {
            vscode.window.showInformationMessage('No docs link available for this item');
        }
        qp.hide();
    });
    qp.onDidTriggerButton(async () => {
        // Open scope picker
        try {
            const cfg = vscode.workspace.getConfiguration('ruggle');
            const scopes: string[] = await withPortRecovery(cfg, async (h) => fetchJson(`${h}/scopes`));
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Ruggle Scope',
                placeHolder: scope,
            });
            if (picked) {
                scope = picked;
                outChan?.appendLine(`[Ruggle] Scope changed to ${scope}`);
                if (qp.value.length >= 2) {
                    // re-run search immediately with new scope
                    if (handle) clearTimeout(handle);
                    handle = setTimeout(() => run(qp.value), 10);
                }
            }
        } catch (e: unknown) {
            vscode.window.showErrorMessage(`Ruggle error: ${errorMessage(e)}`);
        }
    });
    qp.onDidHide(() => qp.dispose());
    qp.show();
    run(query);
}

function stopServer() {
    if (serverProc) {
        try { serverProc.kill('SIGTERM'); } catch { /* noop */ }
        serverProc = undefined;
        outChan?.appendLine('[Ruggle] Server stopped');
        return true;
    }
    outChan?.appendLine('[Ruggle] No server process to stop');
    return false;
}

async function ensureServer(cfg: vscode.WorkspaceConfiguration): Promise<boolean> {
    const host: string = cfg.get('host', 'http://localhost:8000');
    let effectiveHost = host;
    if (await isHealthy(effectiveHost)) return true;
    const auto: boolean = cfg.get('autoStart', true);
    if (!auto) return false;
    const mode: string = cfg.get('serverMode', 'managed');
    const indexDir: string = cfg.get('indexDir', '');
    const repoRoot: string = cfg.get('repoRoot', '');
    const port: number = cfg.get('port', 8000);
    const cmdPath: string = cfg.get('serverCommand', '');
    const managedServerUrl: string = cfg.get('managed.serverUrl', '');
    const managedIndexUrl: string = cfg.get('managed.indexUrl', '');

    try {
        // Common configuration for local modes (managed, cargo, binary)
        const idx = indexDir || ensureUserRuggleIndexDir();
        const portFile = path.join(getStoragePath(), 'port.json');
        const commonSrvArgs: string[] = ['--host', '127.0.0.1', '--port', '0', '--port-file', portFile];
        if (idx) { commonSrvArgs.push('--index', idx); }

        if (mode === 'managed') {
            const bin = await installManagedBinary(managedServerUrl);
            const args: string[] = [...commonSrvArgs];
            serverProc = spawn(bin, args);
            outChan?.appendLine(`[Ruggle] Starting managed server: ${bin} ${args.join(' ')}`);
            // Wait for port-file and update ruggle.host
            const url = await waitForPortFile(portFile, 10000);
            if (url) {
                await cfg.update('host', url, vscode.ConfigurationTarget.Global);
                outChan?.appendLine(`[Ruggle] Server URL set to ${url}`);
                effectiveHost = url;
            } else {
                outChan?.appendLine('[Ruggle] Port file not found in time');
            }
        } else if (mode === 'cargo') {
            const cargoArgs = ['run', '-p', 'ruggle-server', '--bin', 'ruggle-server', '--release', '--', ...commonSrvArgs];
            const options: { cwd?: string } = {};
            if (repoRoot) options.cwd = repoRoot;
            outChan?.appendLine(`[Ruggle] Starting cargo server in ${options.cwd || process.cwd()}`);
            outChan?.appendLine(`[Ruggle] Cargo args: ${cargoArgs.join(' ')}`);
            outChan?.appendLine(`[Ruggle] Options: ${JSON.stringify(options)}`);

            serverProc = spawn('cargo', cargoArgs, options);
            outChan?.appendLine(`[Ruggle] Starting server: cargo ${cargoArgs.join(' ')}`);
            // Wait for port-file and update ruggle.host
            const url = await waitForPortFile(portFile, 10000);
            if (url) {
                await cfg.update('host', url, vscode.ConfigurationTarget.Global);
                outChan?.appendLine(`[Ruggle] Server URL set to ${url}`);
                effectiveHost = url;
            } else {
                outChan?.appendLine('[Ruggle] Port file not found in time');
            }
        } else if (mode === 'binary' && cmdPath) {
            const args: string[] = [...commonSrvArgs];
            serverProc = spawn(cmdPath, args);
            outChan?.appendLine(`[Ruggle] Starting server: ${cmdPath} ${args.join(' ')}`);
            // Wait for port-file and update ruggle.host
            const url = await waitForPortFile(portFile, 10000);
            if (url) {
                await cfg.update('host', url, vscode.ConfigurationTarget.Global);
                outChan?.appendLine(`[Ruggle] Server URL set to ${url}`);
                effectiveHost = url;
            } else {
                outChan?.appendLine('[Ruggle] Port file not found in time');
            }
        } else if (mode === 'docker') {
            const idx = indexDir || ensureUserRuggleIndexDir();
            const args = ['run', '--rm', '-p', `${port}:8000`];
            if (idx) { args.push('-v', `${idx}:/ruggle-index`); }
            args.push('ghcr.io/your-org/ruggle:latest');
            serverProc = spawn('docker', args);
            outChan?.appendLine(`[Ruggle] Starting server: docker ${args.join(' ')}`);
        }
    } catch (e: unknown) {
        outChan?.appendLine(`[Ruggle] Failed to start server: ${errorMessage(e)}`);
    }

    if (serverProc) {
        serverProc.stdout?.on('data', (d) => outChan?.append(`[server] ${d}`));
        serverProc.stderr?.on('data', (d) => outChan?.append(`[server] ${d}`));
    }

    for (let i = 0; i < 40; i++) {
        if (await isHealthy(effectiveHost)) {
            outChan?.appendLine('[Ruggle] Server is ready');
            return true;
        }
        await new Promise(r => setTimeout(r, 300));
    }
    outChan?.appendLine('[Ruggle] Server did not become ready in time');
    return false;
}

async function isHealthy(host: string): Promise<boolean> {
    try {
        const res = await fetch(`${host}/scopes`, { signal: AbortSignal.timeout(1000) });
        return res.ok;
    } catch {
        return false;
    }
}

async function installManagedBinary(explicitUrl?: string): Promise<string> {
    const storage = getStoragePath();
    const binDir = path.join(storage, 'server');
    const platform = os.platform();
    const arch = os.arch();
    const binName = platform === 'win32' ? 'ruggle-server.exe' : 'ruggle-server';
    const binPath = path.join(binDir, binName);
    if (fs.existsSync(binPath)) return binPath;

    await fs.promises.mkdir(binDir, { recursive: true });
    if (explicitUrl) {
        await downloadFile(explicitUrl, `${binPath}.download`);
    } else {
        const candidates = getServerCandidateUrls(platform, arch);
        let ok = false;
        for (const url of candidates) {
            try {
                outChan?.appendLine(`[Ruggle] Trying server download: ${url}`);
                await downloadFile(url, `${binPath}.download`);
                ok = true;
                break;
            } catch (e: unknown) {
                outChan?.appendLine(`[Ruggle] Download failed: ${errorMessage(e)}`);
            }
        }
        if (!ok) throw new Error('No suitable server binary found for this platform');
    }
    await fs.promises.rename(`${binPath}.download`, binPath);
    if (platform !== 'win32') await fs.promises.chmod(binPath, 0o755);
    return binPath;
}

async function installManagedIndex(explicitUrl?: string): Promise<string | undefined> {
    if (!explicitUrl) return undefined;
    const storage = getStoragePath();
    const idxDir = path.join(storage, 'index');
    await fs.promises.mkdir(idxDir, { recursive: true });
    const archivePath = path.join(idxDir, 'index.tgz');
    await downloadFile(explicitUrl, archivePath);
    // naive extract: rely on server to read the directory if you unpack; here we keep archive path not used.
    return idxDir;
}

function getStoragePath(): string {
    const ext = vscode.extensions.getExtension('AlperenKeles.ruggle');
    const storage = ext?.extensionPath ? path.join(ext.extensionPath, '.ruggle') : os.tmpdir();
    if (!fs.existsSync(storage)) fs.mkdirSync(storage, { recursive: true });
    return storage;
}

async function downloadFile(url: string, dest: string): Promise<void> {
    const controller = new AbortController();
    const res = await fetch(url, { signal: controller.signal });
    if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
    const buf = await res.arrayBuffer();
    await fs.promises.writeFile(dest, Buffer.from(buf));
}

function getServerCandidateUrls(platform: NodeJS.Platform, arch: string): string[] {
    const latest = 'https://github.com/alpaylan/ruggle/releases/latest/download';
    const v010 = 'https://github.com/alpaylan/ruggle/releases/download/v0.1.0';
    const names: string[] = [];
    if (platform === 'darwin') {
        // Prefer arm64 if added in future, then x86_64
        if (arch === 'arm64') {
            names.push('ruggle-server-aarch64-apple-darwin');
        }
        names.push('ruggle-server-x86_64-apple-darwin');
    } else if (platform === 'win32') {
        names.push('ruggle-server-x86_64-pc-windows-msvc.exe');
    } else {
        // linux default
        names.push('ruggle-server-x86_64-unknown-linux-gnu');
    }
    const latestUrls = names.map(n => `${latest}/${n}`);
    const versionedUrls = names.map(n => `${v010}/${n}`);
    return [...latestUrls, ...versionedUrls];
}

async function waitForPortFile(portFile: string, timeoutMs: number): Promise<string | undefined> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
        try {
            const data = await fs.promises.readFile(portFile, 'utf8');
            const json = JSON.parse(data);
            if (json && typeof json.url === 'string' && json.url.startsWith('http')) {
                return json.url;
            }
        } catch { }
        await new Promise((r) => setTimeout(r, 200));
    }
    return undefined;
}

function ensureUserRuggleIndexDir(): string {
    const home = os.homedir();
    const base = path.join(home, '.ruggle');
    const crateDir = path.join(base, 'crate');
    try {
        if (!fs.existsSync(base)) fs.mkdirSync(base, { recursive: true });
        if (!fs.existsSync(crateDir)) fs.mkdirSync(crateDir, { recursive: true });
    } catch (e) {
        outChan?.appendLine(`[Ruggle] Failed to ensure ~/.ruggle directory: ${e}`);
    }
    return base;
}

// Port recovery helpers
function getPortFilePath(): string {
    return path.join(getStoragePath(), 'port.json');
}

async function readPortFileNow(): Promise<string | undefined> {
    try {
        const data = await fs.promises.readFile(getPortFilePath(), 'utf8');
        const json = JSON.parse(data);
        if (json && typeof json.url === 'string' && json.url.startsWith('http')) {
            return json.url;
        }
    } catch { }
    return undefined;
}

async function checkPortFileAndUpdateHost(cfg: vscode.WorkspaceConfiguration): Promise<string | undefined> {
    const url = await readPortFileNow();
    if (!url) return undefined;
    const current = cfg.get('host', 'http://localhost:8000');
    if (url !== current) {
        await cfg.update('host', url, vscode.ConfigurationTarget.Global);
        outChan?.appendLine(`[Ruggle] Host updated from port file: ${url}`);
    }
    return url;
}

async function withPortRecovery<T>(cfg: vscode.WorkspaceConfiguration, attempt: (host: string) => Promise<T>): Promise<T> {
    const initial = cfg.get('host', 'http://localhost:8000');
    try {
        return await attempt(initial);
    } catch (e) {
        const updated = await checkPortFileAndUpdateHost(cfg);
        const nextHost = updated || cfg.get('host', initial);
        if (nextHost && nextHost !== initial) {
            return attempt(nextHost);
        }
        throw e;
    }
}


