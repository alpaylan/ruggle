"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = __importStar(require("vscode"));
const https = __importStar(require("https"));
const http = __importStar(require("http"));
const node_child_process_1 = require("node:child_process");
async function fetchJson(url, signal) {
    // Prefer VS Code's global fetch when available
    if (typeof fetch !== 'undefined') {
        const res = await fetch(url, { signal });
        if (!res.ok)
            throw new Error(`${res.status} ${res.statusText}`);
        return res.json();
    }
    // Fallback to Node http/https
    const client = url.startsWith('https') ? https : http;
    return new Promise((resolve, reject) => {
        const req = client.get(url, (res) => {
            const status = res.statusCode ?? 0;
            if (status < 200 || status >= 300) {
                reject(new Error(`${status}`));
                return;
            }
            const chunks = [];
            res.on('data', (c) => chunks.push(c));
            res.on('end', () => {
                try {
                    const text = Buffer.concat(chunks).toString('utf8');
                    resolve(JSON.parse(text));
                }
                catch (e) {
                    reject(e);
                }
            });
        });
        req.on('error', reject);
        if (signal)
            signal.addEventListener('abort', () => req.destroy(new Error('AbortError')));
        req.end();
    });
}
let outChan;
let serverProc;
function activate(context) {
    outChan = vscode.window.createOutputChannel('Roogle');
    const searchCmd = vscode.commands.registerCommand('roogle.search', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        // Ensure server is reachable or start it if configured
        const ok = await ensureServer(cfg);
        if (!ok) {
            vscode.window.showErrorMessage('Roogle server is not reachable. Configure roogle.host or enable autoStart.');
            return;
        }
        const host = cfg.get('host', 'http://localhost:8000');
        const scope = cfg.get('scope', 'set:libstd');
        const limit = cfg.get('limit', 30);
        const threshold = cfg.get('threshold', 0.4);
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
        const host = cfg.get('host', 'http://localhost:8000');
        const scope = cfg.get('scope', 'set:libstd');
        const limit = cfg.get('limit', 30);
        const threshold = cfg.get('threshold', 0.4);
        await presentSearch(host, scope, selected, limit, threshold);
    });
    const setHostCmd = vscode.commands.registerCommand('roogle.setHost', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const current = cfg.get('host', 'http://localhost:8000');
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
        const host = cfg.get('host', 'http://localhost:8000');
        const current = cfg.get('scope', 'set:libstd');
        try {
            const scopes = await fetchJson(`${host}/scopes`);
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Roogle: Set Scope',
                placeHolder: current,
            });
            if (picked) {
                await cfg.update('scope', picked, vscode.ConfigurationTarget.Global);
                vscode.window.showInformationMessage(`Roogle scope set to ${picked}`);
            }
        }
        catch (e) {
            vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
        }
    });
    const startServerCmd = vscode.commands.registerCommand('roogle.startServer', async () => {
        const cfg = vscode.workspace.getConfiguration('roogle');
        const ok = await ensureServer(cfg);
        vscode.window.showInformationMessage(ok ? 'Roogle server is running' : 'Failed to start Roogle server');
    });
    const stopServerCmd = vscode.commands.registerCommand('roogle.stopServer', async () => {
        stopServer();
        vscode.window.showInformationMessage('Roogle server stopped');
    });
    const showLogsCmd = vscode.commands.registerCommand('roogle.showLogs', async () => {
        outChan?.show(true);
        outChan?.appendLine('[Roogle] Logs opened');
    });
    context.subscriptions.push(searchCmd, searchSelCmd, setHostCmd, setScopeCmd, startServerCmd, stopServerCmd, showLogsCmd);
}
function deactivate() { }
async function presentSearch(host, initialScope, query, limit, threshold) {
    const qp = vscode.window.createQuickPick();
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
    let handle;
    let tokenSrc = new AbortController();
    async function run(q) {
        if (q.length < 2) {
            qp.items = [];
            return;
        }
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
            outChan?.appendLine(`[Roogle] GET ${host}/search?...`);
            let hits = null;
            try {
                hits = await fetchJson(getUrl, tokenSrc.signal);
            }
            catch (getErr) {
                // Retry with POST body if GET fails (e.g., due to proxies or URL limits)
                if (typeof fetch !== 'undefined') {
                    const postUrl = `${host}/search?${params.toString()}&scope=${encodeURIComponent(scope)}`;
                    outChan?.appendLine(`[Roogle] POST fallback ${host}/search?...`);
                    const res = await fetch(postUrl, {
                        method: 'POST',
                        body: q,
                        signal: tokenSrc.signal,
                        headers: { 'Content-Type': 'text/plain; charset=utf-8' },
                    });
                    if (!res.ok)
                        throw new Error(`${res.status} ${res.statusText}`);
                    hits = await res.json();
                }
                else {
                    throw getErr;
                }
            }
            const safeHits = hits ?? [];
            const mapped = safeHits.map((h) => ({
                label: h.signature || h.name || '',
                description: (h.path || []).join('::'),
                detail: '',
                link: `https://doc.rust-lang.org/${(h.link || []).join('/')}`,
                alwaysShow: true,
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
        }
        catch (e) {
            if (e.name !== 'AbortError') {
                vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
                qp.items = [{ label: 'Error fetching results', description: String(e) }];
                outChan?.appendLine(`[Roogle] Error: ${e?.stack || e}`);
            }
        }
        finally {
            qp.busy = false;
        }
    }
    qp.onDidChangeValue((v) => {
        // Always show a hint item so the list isn't visually empty
        if (v.length < 2) {
            qp.items = [{ label: 'Type at least 2 characters…', description: '' }];
        }
        if (handle)
            clearTimeout(handle);
        handle = setTimeout(() => run(v), 300);
    });
    qp.onDidAccept(() => {
        const picked = qp.selectedItems[0];
        if (picked?.link)
            vscode.env.openExternal(vscode.Uri.parse(picked.link));
        qp.hide();
    });
    qp.onDidTriggerButton(async () => {
        // Open scope picker
        try {
            const scopes = await fetchJson(`${host}/scopes`);
            const picked = await vscode.window.showQuickPick(scopes, {
                title: 'Roogle Scope',
                placeHolder: scope,
            });
            if (picked) {
                scope = picked;
                outChan?.appendLine(`[Roogle] Scope changed to ${scope}`);
                if (qp.value.length >= 2) {
                    // re-run search immediately with new scope
                    if (handle)
                        clearTimeout(handle);
                    handle = setTimeout(() => run(qp.value), 10);
                }
            }
        }
        catch (e) {
            vscode.window.showErrorMessage(`Roogle error: ${e.message || e}`);
        }
    });
    qp.onDidHide(() => qp.dispose());
    qp.show();
    run(query);
}
function stopServer() {
    if (serverProc) {
        try {
            serverProc.kill('SIGTERM');
        }
        catch { /* noop */ }
        serverProc = undefined;
        outChan?.appendLine('[Roogle] Server stopped');
    }
}
async function ensureServer(cfg) {
    const host = cfg.get('host', 'http://localhost:8000');
    if (await isHealthy(host))
        return true;
    const auto = cfg.get('autoStart', true);
    if (!auto)
        return false;
    const mode = cfg.get('serverMode', 'cargo');
    const indexDir = cfg.get('indexDir', '');
    const repoRoot = cfg.get('repoRoot', '');
    const port = cfg.get('port', 8000);
    const cmdPath = cfg.get('serverCommand', '');
    try {
        if (mode === 'cargo') {
            const args = ['run', '-p', 'roogle-server', '--bin', 'roogle-server', '--release'];
            if (indexDir)
                args.push('--', '--index', indexDir);
            const options = {};
            if (repoRoot)
                options.cwd = repoRoot;
            serverProc = (0, node_child_process_1.spawn)('cargo', args, options);
            outChan?.appendLine(`[Roogle] Starting server: cargo ${args.join(' ')}`);
        }
        else if (mode === 'binary' && cmdPath) {
            const args = [];
            if (indexDir) {
                args.push('--index', indexDir);
            }
            serverProc = (0, node_child_process_1.spawn)(cmdPath, args);
            outChan?.appendLine(`[Roogle] Starting server: ${cmdPath} ${args.join(' ')}`);
        }
        else if (mode === 'docker') {
            const args = ['run', '--rm', '-p', `${port}:8000`];
            if (indexDir) {
                args.push('-v', `${indexDir}:/roogle-index`);
            }
            args.push('ghcr.io/your-org/roogle:latest');
            serverProc = (0, node_child_process_1.spawn)('docker', args);
            outChan?.appendLine(`[Roogle] Starting server: docker ${args.join(' ')}`);
        }
    }
    catch (e) {
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
async function isHealthy(host) {
    try {
        const res = await fetch(`${host}/scopes`, { signal: AbortSignal.timeout(1000) });
        return res.ok;
    }
    catch {
        return false;
    }
}
//# sourceMappingURL=extension.js.map