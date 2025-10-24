import * as vscode from 'vscode';
import * as https from 'https';
import * as http from 'http';
import { spawn, ChildProcess } from 'node:child_process';
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

export function activate(context: vscode.ExtensionContext) {
    outChan = vscode.window.createOutputChannel('Roogle');
    const searchCmd = vscode.commands.registerCommand('roogle.search', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        // Ensure server is reachable or start it if configured
        const ok = await ensureServer(cfg);
        if (!ok) {
            vscode.window.showErrorMessage('Roogle server is not reachable. Configure roogle.host or enable autoStart.');
            return;
        }
        const host: string = cfg.get('host', 'http://localhost:8000');
        const scope: string = cfg.get('scope', 'set:libstd');
        const limit: number = cfg.get('limit', 30);
        const threshold: number = cfg.get('threshold', 0.4);
        outChan?.appendLine(`[Roogle] Opening QuickPick (dynamic search) host=${host} scope=${scope}`);
        await presentSearch(host, scope, '', limit, threshold);
    });

    const searchSelCmd = vscode.commands.registerCommand('roogle.searchSelection', async () => {
        const editor = vscode.window.activeTextEditor;
        const selected = editor?.document.getText(editor.selection).trim();
        if (!selected) {
            vscode.window.showInformationMessage('No selection to search');
            return;
        }
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        const scope: string = cfg.get('scope', 'set:libstd');
        const limit: number = cfg.get('limit', 30);
        const threshold: number = cfg.get('threshold', 0.4);
        await presentSearch(host, scope, selected, limit, threshold);
    });

    const setHostCmd = vscode.commands.registerCommand('roogle.setHost', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const current: string = cfg.get('host', 'http://localhost:8000');
        const input = await vscode.window.showInputBox({
            prompt: 'Roogle server host URL',
            value: current,
        });
        if (input) {
            await cfg.update('host', input, vscode.ConfigurationTarget.Global);
            vscode.window.showInformationMessage(`Roogle host set to ${input}`);
        }
    });

    const setScopeCmd = vscode.commands.registerCommand('roogle.setScope', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        const current: string = cfg.get('scope', 'set:libstd');
        try {
            const scopes: string[] = await fetchJson(`${host}/scopes`);
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Roogle: Set Scope',
                placeHolder: current,
            });
            if (picked) {
                await cfg.update('scope', picked, vscode.ConfigurationTarget.Global);
                vscode.window.showInformationMessage(`Roogle scope set to ${picked}`);
            }
        } catch (e: any) {
            vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
        }
    });

    const startServerCmd = vscode.commands.registerCommand('roogle.startServer', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const ok = await ensureServer(cfg);
        vscode.window.showInformationMessage(ok ? 'Roogle server is running' : 'Failed to start Roogle server');
    });
    const stopServerCmd = vscode.commands.registerCommand('roogle.stopServer', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        try {
            await fetch(`${host}/stop`, { method: 'POST', signal: AbortSignal.timeout(1000) });
            outChan?.appendLine('[Roogle] Sent /stop');
        } catch (e: any) {
            outChan?.appendLine(`[Roogle] /stop failed: ${e?.message || e}`);
        }
    });

    const updateIndexCmd = vscode.commands.registerCommand('roogle.updateIndex', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
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
            const target = `${host}/index`;
            outChan?.appendLine(`[Roogle] POST ${target} scopes=${JSON.stringify(scopes)}`);
            const res = await fetch(target, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(scopes),
                signal: AbortSignal.timeout(30000),
            });
            const text = await res.text();
            if (!res.ok) throw new Error(text || res.statusText);
            vscode.window.showInformationMessage(`Index update: ${text}`);
            outChan?.appendLine(`[Roogle] Index update response: ${text}`);
        } catch (e: any) {
            vscode.window.showErrorMessage(`Index update failed: ${e?.message || e}`);
            outChan?.appendLine(`[Roogle] Index update failed: ${e?.stack || e}`);
        }
    });

    const installLibstdCmd = vscode.commands.registerCommand('roogle.installLibstdIndex', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        const base = 'https://raw.githubusercontent.com/alpaylan/roogle-index/main/crate';
        const names = ['std', 'core', 'alloc'];
        const urls = names.flatMap(n => [`${base}/${n}.bin`, `${base}/${n}.json`]);
        try {
            const res = await fetch(`${host}/index`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ urls }),
                signal: AbortSignal.timeout(20000),
            });
            const text = await res.text();
            if (!res.ok) throw new Error(text || res.statusText);
            vscode.window.showInformationMessage(`Index update: ${text}`);
            outChan?.appendLine(`[Roogle] Index install libstd response: ${text}`);
        } catch (e: any) {
            vscode.window.showErrorMessage(`Index install failed: ${e?.message || e}`);
            outChan?.appendLine(`[Roogle] Index install failed: ${e?.stack || e}`);
        }
    });

    const showLogsCmd = vscode.commands.registerCommand('roogle.showLogs', async () => {
        outChan?.show(true);
        outChan?.appendLine('[Roogle] Logs opened');
    });

    const listIndexedCmd = vscode.commands.registerCommand('roogle.listIndexed', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const host: string = cfg.get('host', 'http://localhost:8000');
        try {
            const names: string[] = await fetchJson(`${host}/index`);
            const picked = await vscode.window.showQuickPick(names, { title: 'Indexed Crates' });
            if (picked) { vscode.env.clipboard.writeText(picked); }
        } catch (e: any) {
            vscode.window.showErrorMessage(`Failed to list indexed: ${e?.message || e}`);
        }
    });

    context.subscriptions.push(searchCmd, searchSelCmd, setHostCmd, setScopeCmd, startServerCmd, stopServerCmd, updateIndexCmd, installLibstdCmd, listIndexedCmd, showLogsCmd);
}

export function deactivate() { }

async function presentSearch(host: string, initialScope: string, query: string, limit: number, threshold: number) {
    const qp = vscode.window.createQuickPick<vscode.QuickPickItem & { link?: string }>();
    qp.matchOnDescription = true;
    qp.matchOnDetail = true;
    qp.placeholder = 'Type to refine results…';
    qp.title = 'Roogle Results';
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
        outChan?.appendLine(`[Roogle] run q='${q.slice(0, 120)}'… limit=${limit} threshold=${threshold} scope=${scope}`);
        tokenSrc.abort();
        tokenSrc = new AbortController();
        const params = new URLSearchParams();
        params.set('scope', scope);
        params.set('limit', String(limit));
        params.set('threshold', String(threshold));
        qp.busy = true;
        try {
            const getUrl = `${host}/search?${params.toString()}&query=${encodeURIComponent(q)}`;
            outChan?.appendLine(`[Roogle] GET ${host}/search?${params.toString()}&query=${encodeURIComponent(q)}`);
            let hits: any[] | null = null;
            try {
                hits = await fetchJson<any[]>(getUrl, tokenSrc.signal);
            } catch (getErr: any) {
                // Retry with POST body if GET fails (e.g., due to proxies or URL limits)
                outChan?.appendLine(`[Roogle] GET failed: ${getErr?.message || getErr}`);
                if (typeof fetch !== 'undefined') {
                    const postUrl = `${host}/search?${params.toString()}&query=${encodeURIComponent(q)}`;
                    outChan?.appendLine(`[Roogle] POST fallback ${host}/search?${params.toString()}&query=${encodeURIComponent(q)}`);
                    const res = await fetch(postUrl, {
                        method: 'POST',
                        body: q,
                        signal: tokenSrc.signal,
                        headers: { 'Content-Type': 'text/plain; charset=utf-8' },
                    });
                    if (!res.ok) throw new Error(`${res.status} ${res.statusText} ${await res.text()}`);
                    hits = await res.json();
                } else {
                    throw getErr;
                }
            }
            const safeHits = hits ?? [];
            outChan?.appendLine(`[Roogle] Hits: ${JSON.stringify(hits)}`);
            const mapped = safeHits.map((h: any) => ({
                label: h.signature || h.name || '',
                description: (h.path || []).join('::'),
                detail: '',
                link: `https://doc.rust-lang.org/${h.link}`,
                alwaysShow: true as boolean,
            }));
            qp.items = mapped;
            if (mapped.length > 0) {
                qp.activeItems = [mapped[0]];
            }
            outChan?.appendLine(`[Roogle] Results: ${safeHits.length}`);
            const preview = mapped
                .slice(0, 5)
                .map(i => `${i.label} (${i.description})`)
                .join(' | ');
            outChan?.appendLine(`[Roogle] First items: ${preview}`);
        } catch (e: any) {
            if (e.name !== 'AbortError') {
                vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
                qp.items = [{ label: 'Error fetching results', description: String(e) }];
                outChan?.appendLine(`[Roogle] Error: ${e?.stack || e}`);
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
        const picked = qp.selectedItems[0] as any;
        if (picked?.link) {
            outChan?.appendLine(`[Roogle] Opening docs: ${picked.link}`);
            vscode.env.openExternal(vscode.Uri.parse(picked.link));
        } else {
            vscode.window.showInformationMessage('No docs link available for this item');
        }
        qp.hide();
    });
    qp.onDidTriggerButton(async () => {
        // Open scope picker
        try {
            const scopes: string[] = await fetchJson(`${host}/scopes`);
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Roogle Scope',
                placeHolder: scope,
            });
            if (picked) {
                scope = picked;
                outChan?.appendLine(`[Roogle] Scope changed to ${scope}`);
                if (qp.value.length >= 2) {
                    // re-run search immediately with new scope
                    if (handle) clearTimeout(handle);
                    handle = setTimeout(() => run(qp.value), 10);
                }
            }
        } catch (e: any) {
            vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
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
        outChan?.appendLine('[Roogle] Server stopped');
        return true;
    } else {
        outChan?.appendLine('[Roogle] No server process to stop');
        return false;
    }
}

async function ensureServer(cfg: vscode.WorkspaceConfiguration): Promise<boolean> {
    const host: string = cfg.get('host', 'http://localhost:8000');
    if (await isHealthy(host)) return true;
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
        if (mode === 'managed') {
            const bin = await installManagedBinary(managedServerUrl);
            const userIndexDir = ensureUserRoogleIndexDir();
            const portFile = path.join(getStoragePath(), 'port.json');
            const args: string[] = ['--host', '127.0.0.1', '--port', '0', '--port-file', portFile, '--index', userIndexDir];
            serverProc = spawn(bin, args);
            outChan?.appendLine(`[Roogle] Starting managed server: ${bin} ${args.join(' ')}`);
            // Wait for port-file and update roogle.host
            const url = await waitForPortFile(portFile, 10000);
            if (url) {
                await cfg.update('host', url, vscode.ConfigurationTarget.Global);
                outChan?.appendLine(`[Roogle] Server URL set to ${url}`);
            } else {
                outChan?.appendLine('[Roogle] Port file not found in time');
            }
        } else if (mode === 'cargo') {
            const idx = indexDir || ensureUserRoogleIndexDir();
            const args = ['run', '-p', 'roogle-server', '--bin', 'roogle-server', '--release'];
            if (idx) args.push('--', '--index', idx);
            const options: any = {};
            if (repoRoot) options.cwd = repoRoot;
            serverProc = spawn('cargo', args, options);
            outChan?.appendLine(`[Roogle] Starting server: cargo ${args.join(' ')}`);
        } else if (mode === 'binary' && cmdPath) {
            const idx = indexDir || ensureUserRoogleIndexDir();
            const args: string[] = [];
            if (idx) { args.push('--index', idx); }
            serverProc = spawn(cmdPath, args);
            outChan?.appendLine(`[Roogle] Starting server: ${cmdPath} ${args.join(' ')}`);
        } else if (mode === 'docker') {
            const idx = indexDir || ensureUserRoogleIndexDir();
            const args = ['run', '--rm', '-p', `${port}:8000`];
            if (idx) { args.push('-v', `${idx}:/roogle-index`); }
            args.push('ghcr.io/your-org/roogle:latest');
            serverProc = spawn('docker', args);
            outChan?.appendLine(`[Roogle] Starting server: docker ${args.join(' ')}`);
        }
    } catch (e: any) {
        outChan?.appendLine(`[Roogle] Failed to start server: ${e?.message || e}`);
    }

    if (serverProc) {
        serverProc.stdout?.on('data', (d) => outChan?.append(`[server] ${d}`));
        serverProc.stderr?.on('data', (d) => outChan?.append(`[server] ${d}`));
    }

    for (let i = 0; i < 40; i++) {
        if (await isHealthy(host)) {
            outChan?.appendLine('[Roogle] Server is ready');
            return true;
        }
        await new Promise(r => setTimeout(r, 300));
    }
    outChan?.appendLine('[Roogle] Server did not become ready in time');
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
    const binName = platform === 'win32' ? 'roogle-server.exe' : 'roogle-server';
    const binPath = path.join(binDir, binName);
    if (fs.existsSync(binPath)) return binPath;

    await fs.promises.mkdir(binDir, { recursive: true });
    if (explicitUrl) {
        await downloadFile(explicitUrl, binPath + '.download');
    } else {
        const candidates = getServerCandidateUrls(platform, arch);
        let ok = false;
        for (const url of candidates) {
            try {
                outChan?.appendLine(`[Roogle] Trying server download: ${url}`);
                await downloadFile(url, binPath + '.download');
                ok = true;
                break;
            } catch (e: any) {
                outChan?.appendLine(`[Roogle] Download failed: ${e?.message || e}`);
            }
        }
        if (!ok) throw new Error('No suitable server binary found for this platform');
    }
    await fs.promises.rename(binPath + '.download', binPath);
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
    const ext = vscode.extensions.getExtension('AlperenKeles.roogle');
    const storage = ext?.extensionPath ? path.join(ext.extensionPath, '.roogle') : os.tmpdir();
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
    const latest = 'https://github.com/alpaylan/roogle/releases/latest/download';
    const v010 = 'https://github.com/alpaylan/roogle/releases/download/v0.1.0';
    const names: string[] = [];
    if (platform === 'darwin') {
        // Prefer arm64 if added in future, then x86_64
        if (arch === 'arm64') {
            names.push('roogle-server-aarch64-apple-darwin');
        }
        names.push('roogle-server-x86_64-apple-darwin');
    } else if (platform === 'win32') {
        names.push('roogle-server-x86_64-pc-windows-msvc.exe');
    } else {
        // linux default
        names.push('roogle-server-x86_64-unknown-linux-gnu');
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

function ensureUserRoogleIndexDir(): string {
    const home = os.homedir();
    const base = path.join(home, '.roogle');
    const crateDir = path.join(base, 'crate');
    try {
        if (!fs.existsSync(base)) fs.mkdirSync(base, { recursive: true });
        if (!fs.existsSync(crateDir)) fs.mkdirSync(crateDir, { recursive: true });
    } catch (e) {
        outChan?.appendLine(`[Roogle] Failed to ensure ~/.roogle directory: ${e}`);
    }
    return base;
}


