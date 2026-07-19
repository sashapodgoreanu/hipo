import { useEffect, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X, Loader2, Check, ChevronDown, ChevronRight, Minus, Plus } from 'lucide-react';
import {
    settingsGetProxy,
    settingsSetProxy,
    settingsGetAi,
    settingsSetAi,
    settingsGetRunnerResources,
    settingsSetRunnerResources,
    settingsGetAllowUnsigned,
    settingsSetAllowUnsigned,
    settingsGetContextFile,
    settingsSetContextFile,
    type RunnerAutomaticOrNumber,
    type RunnerResourceLimit,
    type RunnerResourcesProfile,
} from '../tauri-bridge';
import { loadPersisted, savePersisted } from '../persistence';
import {
    DEFAULT_FONT_SIZE,
    MAX_FONT_SIZE,
    MIN_FONT_SIZE,
    getFontSize,
    setFontSize as applyAndSaveFontSize,
} from '../font-size';

const defaultRunnerResources = (): RunnerResourcesProfile => ({
    version: 1,
    memory: { mode: 'automatic' },
    cpuThreads: { mode: 'automatic' },
    spill: { mode: 'automatic' },
    quackParallelism: { mode: 'automatic' },
    baseCapacity: 3,
});

export const positiveNumber = (value: string, fallback: number) => {
    const parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
};

export const clampConcurrentQueries = (value: string) => Math.min(8, positiveNumber(value, 1));

const limitNumber = (limit: RunnerResourceLimit, unit: 'bytes' | 'percent') =>
    limit.mode === unit ? String(unit === 'bytes' ? Math.ceil(limit.value / (1024 * 1024)) : limit.value) : '';

const concurrentNumber = (value: RunnerAutomaticOrNumber) =>
    value.mode === 'value' ? String(value.value) : '';

/**
 * App settings, grouped into collapsible categories so the panel stays simple
 * (#102). Workspace settings (proxy, memory cap, context file, AI endpoint) are
 * persisted per workspace to .duckle/settings.json via the Save button; UI
 * preferences (font size, Dives button) apply immediately and live in
 * localStorage.
 */
export function SettingsModal({
    workspace,
    onClose,
}: {
    workspace: string | null;
    onClose: () => void;
}) {
    const [proxy, setProxy] = useState('');
    // #92: external OpenAI-compatible AI endpoint for the Duckie assistant.
    const [aiBaseUrl, setAiBaseUrl] = useState('');
    const [aiModel, setAiModel] = useState('');
    const [aiKey, setAiKey] = useState('');
    const [runnerResources, setRunnerResources] = useState<RunnerResourcesProfile>(defaultRunnerResources);
    const [runnerDiagnostics, setRunnerDiagnostics] = useState<string[]>([]);
    // #143: allow loading unsigned / community DuckDB extensions (off by default).
    const [allowUnsigned, setAllowUnsigned] = useState(false);
    // Global context file: a key/value file auto-merged into the global context.
    const [contextFile, setContextFile] = useState('');
    // Local UI pref: show/hide the top-bar Dives button.
    const [showDives, setShowDives] = useState(() => !loadPersisted('hideDivesButton', false));
    // Local UI pref: global font size (applies live, no Save).
    const [fontSize, setFontSize] = useState(() => getFontSize());
    const [loaded, setLoaded] = useState(false);
    const [saving, setSaving] = useState(false);
    const [saved, setSaved] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Which categories are expanded. Persisted so the panel reopens as left.
    const [expanded, setExpanded] = useState<Set<string>>(
        () => new Set(loadPersisted<string[]>('settingsExpanded', ['appearance'])),
    );

    useEffect(() => {
        let alive = true;
        if (!workspace) {
            setLoaded(true);
            return;
        }
        Promise.all([
            settingsGetProxy(workspace),
            settingsGetAi(workspace),
            settingsGetRunnerResources(workspace),
            settingsGetContextFile(workspace),
            settingsGetAllowUnsigned(workspace),
        ])
            .then(([p, ai, resources, ic, unsigned]) => {
                if (!alive) return;
                setProxy(p ?? '');
                setAiBaseUrl(ai.baseUrl ?? '');
                setAiModel(ai.model ?? '');
                setAiKey(ai.apiKey ?? '');
                setRunnerResources(resources.requested);
                setRunnerDiagnostics(resources.diagnostics);
                setContextFile(ic ?? '');
                setAllowUnsigned(unsigned ?? false);
                setLoaded(true);
            })
            .catch(e => {
                if (alive) {
                    setError(String(e));
                    setLoaded(true);
                }
            });
        return () => {
            alive = false;
        };
    }, [workspace]);

    const save = async () => {
        if (!workspace) return;
        setSaving(true);
        setError(null);
        setSaved(false);
        try {
            await settingsSetProxy(workspace, proxy.trim() || null);
            await settingsSetAi(workspace, {
                baseUrl: aiBaseUrl.trim() || null,
                model: aiModel.trim() || null,
                apiKey: aiKey.trim() || null,
            });
            const resources = await settingsSetRunnerResources(workspace, runnerResources);
            setRunnerResources(resources.requested);
            setRunnerDiagnostics(resources.diagnostics);
            await settingsSetAllowUnsigned(workspace, allowUnsigned);
            await settingsSetContextFile(workspace, contextFile.trim() || null);
            setSaved(true);
            setTimeout(() => setSaved(false), 1500);
        } catch (e) {
            setError(String(e));
        } finally {
            setSaving(false);
        }
    };

    // Local UI pref - applies immediately (no Save), broadcast so App re-reads.
    const toggleDives = (next: boolean) => {
        setShowDives(next);
        savePersisted('hideDivesButton', !next);
        window.dispatchEvent(new Event('duckle:dives-visibility'));
    };

    // Font size applies live as it changes; clamped + persisted in font-size.ts.
    const changeFontSize = (next: number) => {
        setFontSize(applyAndSaveFontSize(next));
    };

    const toggleSection = (id: string) => {
        setExpanded(prev => {
            const next = new Set(prev);
            if (next.has(id)) next.delete(id);
            else next.add(id);
            savePersisted('settingsExpanded', [...next]);
            return next;
        });
    };

    const handleBackdrop = (e: React.MouseEvent) => {
        if (e.target === e.currentTarget) onClose();
    };
    const btn: React.CSSProperties = {
        padding: '7px 14px',
        borderRadius: 8,
        border: '1px solid var(--border-2, #2a2a2a)',
        background: 'transparent',
        color: 'inherit',
        cursor: 'pointer',
        fontWeight: 600,
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
    };
    const primary: React.CSSProperties = {
        ...btn,
        background: 'var(--accent, #ff7a45)',
        borderColor: 'var(--accent, #ff7a45)',
        color: '#0a0a0a',
    };
    const aiInput: React.CSSProperties = {
        width: '100%',
        padding: '8px 10px',
        borderRadius: 8,
        border: '1px solid var(--border-2, #2a2a2a)',
        background: 'var(--bg-1, #14161c)',
        color: 'inherit',
        boxSizing: 'border-box',
    };
    const help: React.CSSProperties = { marginTop: 0, marginBottom: 8, fontSize: '0.9231rem', opacity: 0.7 };

    const Section = ({ id, title, children }: { id: string; title: string; children: ReactNode }) => {
        const open = expanded.has(id);
        return (
            <div className="settings-section">
                <button
                    type="button"
                    className="settings-section-header"
                    aria-expanded={open}
                    onClick={() => toggleSection(id)}
                >
                    <span className="settings-cat-chevron" aria-hidden="true">
                        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                    </span>
                    <span className="settings-section-title">{title}</span>
                </button>
                {open ? <div className="settings-section-body">{children}</div> : null}
            </div>
        );
    };

    return createPortal(
        <div className="modal-backdrop" onClick={handleBackdrop}>
            <div
                className="modal"
                role="dialog"
                aria-modal="true"
                aria-label="Settings"
                style={{ maxWidth: 480 }}
            >
                <div className="modal-header">
                    <div className="modal-title">Settings</div>
                    <button type="button" className="modal-close" onClick={onClose} aria-label="Close">
                        <X size={16} />
                    </button>
                </div>
                <div className="modal-body">
                    {!workspace ? (
                        <p style={{ fontSize: '0.9231rem', color: 'var(--danger, #ff4d6d)', margin: '0 0 8px' }}>
                            Open a workspace first to save workspace settings.
                        </p>
                    ) : null}
                    {error ? (
                        <p style={{ fontSize: '0.9231rem', color: 'var(--danger, #ff4d6d)', margin: '0 0 8px' }}>
                            {error}
                        </p>
                    ) : null}

                    <Section id="appearance" title="Appearance">
                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>
                            Font size
                        </label>
                        <p style={help}>
                            Scales the interface text. Affects every view. ({MIN_FONT_SIZE}-{MAX_FONT_SIZE}px)
                        </p>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                            <button
                                type="button"
                                style={{ ...btn, padding: '6px 10px' }}
                                onClick={() => changeFontSize(fontSize - 1)}
                                disabled={fontSize <= MIN_FONT_SIZE}
                                aria-label="Decrease font size"
                            >
                                <Minus size={14} />
                            </button>
                            <span style={{ minWidth: 56, textAlign: 'center', fontVariantNumeric: 'tabular-nums' }}>
                                {fontSize}px
                            </span>
                            <button
                                type="button"
                                style={{ ...btn, padding: '6px 10px' }}
                                onClick={() => changeFontSize(fontSize + 1)}
                                disabled={fontSize >= MAX_FONT_SIZE}
                                aria-label="Increase font size"
                            >
                                <Plus size={14} />
                            </button>
                            {fontSize !== DEFAULT_FONT_SIZE ? (
                                <button
                                    type="button"
                                    style={{ ...btn, padding: '6px 10px', marginLeft: 'auto' }}
                                    onClick={() => changeFontSize(DEFAULT_FONT_SIZE)}
                                >
                                    Reset
                                </button>
                            ) : null}
                        </div>
                    </Section>

                    <Section id="proxy" title="HTTP / HTTPS proxy">
                        <p style={help}>
                            Routes REST and cloud-API connectors and the in-app updater through a proxy, so
                            Duckle works behind a corporate proxy without setting a system environment
                            variable. Leave empty for a direct connection.
                        </p>
                        <input
                            id="settings-proxy"
                            type="text"
                            value={proxy}
                            onChange={e => setProxy(e.target.value)}
                            placeholder="http://user:pass@proxy.company.com:8080"
                            disabled={!loaded || !workspace}
                            spellCheck={false}
                            autoComplete="off"
                            style={aiInput}
                        />
                    </Section>

                    <Section id="runner-resources" title="Runner resources">
                        <p style={help}>
                            Settings apply immediately as the desired profile. Active queries complete with
                            their current profile; new queries wait for the atomic latest profile. Base
                            capacity uses the normal elastic pool policy and never terminates leased workers.
                        </p>
                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>Memory per worker</label>
                        <div style={{ display: 'flex', gap: 8, marginBottom: 10 }}>
                            <select
                                value={runnerResources.memory.mode}
                                onChange={e => setRunnerResources(current => ({
                                    ...current,
                                    memory: e.target.value === 'automatic'
                                        ? { mode: 'automatic' }
                                        : e.target.value === 'percent'
                                            ? { mode: 'percent', value: 80 }
                                            : { mode: 'bytes', value: 1024 * 1024 * 1024 },
                                }))}
                                disabled={!loaded || !workspace}
                                style={{ ...aiInput, width: 150 }}
                            >
                                <option value="automatic">Automatic</option>
                                <option value="percent">Percent of host</option>
                                <option value="bytes">Absolute MB</option>
                            </select>
                            {runnerResources.memory.mode !== 'automatic' ? (
                                <input
                                    type="number"
                                    min={1}
                                    max={runnerResources.memory.mode === 'percent' ? 100 : undefined}
                                    value={limitNumber(runnerResources.memory, runnerResources.memory.mode === 'percent' ? 'percent' : 'bytes')}
                                    onChange={e => setRunnerResources(current => ({
                                        ...current,
                                        memory: current.memory.mode === 'percent'
                                            ? { mode: 'percent', value: Math.min(100, positiveNumber(e.target.value, 1)) }
                                            : { mode: 'bytes', value: positiveNumber(e.target.value, 1) * 1024 * 1024 },
                                    }))}
                                    aria-label={runnerResources.memory.mode === 'percent' ? 'Memory percent' : 'Memory MB'}
                                    disabled={!loaded || !workspace}
                                    style={aiInput}
                                />
                            ) : null}
                        </div>

                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>Spill / temporary space</label>
                        <div style={{ display: 'flex', gap: 8, marginBottom: 10 }}>
                            <select
                                value={runnerResources.spill.mode}
                                onChange={e => setRunnerResources(current => ({
                                    ...current,
                                    spill: e.target.value === 'automatic'
                                        ? { mode: 'automatic' }
                                        : e.target.value === 'percent'
                                            ? { mode: 'percent', value: 80 }
                                            : { mode: 'bytes', value: 1024 * 1024 * 1024 },
                                }))}
                                disabled={!loaded || !workspace}
                                style={{ ...aiInput, width: 150 }}
                            >
                                <option value="automatic">Automatic</option>
                                <option value="percent">Percent of disk</option>
                                <option value="bytes">Absolute MB</option>
                            </select>
                            {runnerResources.spill.mode !== 'automatic' ? (
                                <input
                                    type="number"
                                    min={1}
                                    max={runnerResources.spill.mode === 'percent' ? 100 : undefined}
                                    value={limitNumber(runnerResources.spill, runnerResources.spill.mode === 'percent' ? 'percent' : 'bytes')}
                                    onChange={e => setRunnerResources(current => ({
                                        ...current,
                                        spill: current.spill.mode === 'percent'
                                            ? { mode: 'percent', value: Math.min(100, positiveNumber(e.target.value, 1)) }
                                            : { mode: 'bytes', value: positiveNumber(e.target.value, 1) * 1024 * 1024 },
                                    }))}
                                    aria-label={runnerResources.spill.mode === 'percent' ? 'Spill percent' : 'Spill MB'}
                                    disabled={!loaded || !workspace}
                                    style={aiInput}
                                />
                            ) : null}
                        </div>

                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>CPU threads per worker</label>
                        <div style={{ display: 'flex', gap: 8, marginBottom: 10 }}>
                            <select
                                value={runnerResources.cpuThreads.mode}
                                onChange={e => setRunnerResources(current => ({
                                    ...current,
                                    cpuThreads: e.target.value === 'automatic'
                                        ? { mode: 'automatic' }
                                        : { mode: 'value', value: 1 },
                                }))}
                                disabled={!loaded || !workspace}
                                style={{ ...aiInput, width: 150 }}
                            >
                                <option value="automatic">Automatic</option>
                                <option value="value">Fixed threads</option>
                            </select>
                            {runnerResources.cpuThreads.mode === 'value' ? (
                                <input
                                    type="number"
                                    min={1}
                                    value={concurrentNumber(runnerResources.cpuThreads)}
                                    onChange={e => setRunnerResources(current => ({
                                        ...current,
                                        cpuThreads: { mode: 'value', value: positiveNumber(e.target.value, 1) },
                                    }))}
                                    aria-label="CPU threads"
                                    disabled={!loaded || !workspace}
                                    style={aiInput}
                                />
                            ) : null}
                        </div>

                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>Concurrent queries per run</label>
                        <div style={{ display: 'flex', gap: 8, marginBottom: 10 }}>
                            <select
                                value={runnerResources.quackParallelism.mode}
                                aria-label="Concurrent query mode"
                                onChange={e => setRunnerResources(current => ({
                                    ...current,
                                    quackParallelism: e.target.value === 'automatic'
                                        ? { mode: 'automatic' }
                                        : { mode: 'value', value: 1 },
                                }))}
                                disabled={!loaded || !workspace}
                                style={{ ...aiInput, width: 150 }}
                            >
                                <option value="automatic">Automatic (8)</option>
                                <option value="value">Fixed 1–8</option>
                            </select>
                            {runnerResources.quackParallelism.mode === 'value' ? (
                                <input
                                    type="number"
                                    min={1}
                                    max={8}
                                    value={concurrentNumber(runnerResources.quackParallelism)}
                                    onChange={e => setRunnerResources(current => ({
                                        ...current,
                                        quackParallelism: { mode: 'value', value: clampConcurrentQueries(e.target.value) },
                                    }))}
                                    aria-label="Concurrent queries per run"
                                    disabled={!loaded || !workspace}
                                    style={aiInput}
                                />
                            ) : null}
                        </div>

                        <label style={{ display: 'block', fontWeight: 600, marginBottom: 6 }}>Warm worker base capacity</label>
                        <input
                            id="settings-runner-base-capacity"
                            type="number"
                            min={1}
                            value={runnerResources.baseCapacity}
                            onChange={e => setRunnerResources(current => ({
                                ...current,
                                baseCapacity: positiveNumber(e.target.value, 1),
                            }))}
                            disabled={!loaded || !workspace}
                            style={aiInput}
                        />
                        <p style={{ ...help, marginTop: 8 }}>
                            Default 3. The elastic target is max(base, ceil(peak over 5 minutes × 1.20)); this is not a worker limit.
                        </p>
                        {runnerDiagnostics.length > 0 ? (
                            <p style={{ ...help, marginBottom: 0 }}>
                                Effective profile constrained by: {runnerDiagnostics.join(', ').replaceAll('_', ' ')}.
                            </p>
                        ) : null}
                    </Section>

                    <Section id="unsigned" title="Unsigned extensions">
                        <p style={help}>
                            Allow loading unsigned or community DuckDB extensions (for example a custom{' '}
                            <code>quack</code> build). When on, the engine starts DuckDB with{' '}
                            <code>-unsigned</code>. Leave off unless you trust the extension: it turns off
                            signature verification for every run in this workspace.
                        </p>
                        <label style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}>
                            <input
                                type="checkbox"
                                checked={allowUnsigned}
                                onChange={e => setAllowUnsigned(e.target.checked)}
                                disabled={!loaded || !workspace}
                            />
                            Allow unsigned extensions
                        </label>
                    </Section>

                    <Section id="context" title="Global context file">
                        <p style={help}>
                            Auto-load context variables from a key/value file before every run, so{' '}
                            <code>{'${KEY}'}</code> resolves everywhere without wiring a node. Supports .env /
                            .properties (KEY=VALUE), .csv (key,value) and .json. A relative path is resolved
                            against the workspace root.
                        </p>
                        <input
                            id="settings-context-file"
                            type="text"
                            value={contextFile}
                            onChange={e => setContextFile(e.target.value)}
                            placeholder="config/context.env  (or an absolute path)"
                            disabled={!loaded || !workspace}
                            spellCheck={false}
                            autoComplete="off"
                            style={aiInput}
                        />
                    </Section>

                    <Section id="ai" title="AI assistant endpoint">
                        <p style={help}>
                            Point Duckie at an external OpenAI-compatible API (OpenAI, Ollama, LM Studio,
                            vLLM, ...) instead of the bundled local model. Leave the base URL empty to use
                            the local Qwen model.
                        </p>
                        <input
                            type="text"
                            value={aiBaseUrl}
                            onChange={e => setAiBaseUrl(e.target.value)}
                            placeholder="Base URL, e.g. https://api.openai.com"
                            disabled={!loaded || !workspace}
                            spellCheck={false}
                            autoComplete="off"
                            style={aiInput}
                        />
                        <input
                            type="text"
                            value={aiModel}
                            onChange={e => setAiModel(e.target.value)}
                            placeholder="Model, e.g. gpt-4o-mini"
                            disabled={!loaded || !workspace}
                            spellCheck={false}
                            autoComplete="off"
                            style={{ ...aiInput, marginTop: 8 }}
                        />
                        <input
                            type="password"
                            value={aiKey}
                            onChange={e => setAiKey(e.target.value)}
                            placeholder="API key (sent as a Bearer token)"
                            disabled={!loaded || !workspace}
                            spellCheck={false}
                            autoComplete="off"
                            style={{ ...aiInput, marginTop: 8 }}
                        />
                    </Section>

                    <Section id="toolbar" title="Toolbar">
                        <label style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: '1rem', cursor: 'pointer' }}>
                            <input type="checkbox" checked={showDives} onChange={e => toggleDives(e.target.checked)} />
                            Show the Dives button (live data views &amp; dashboards) in the toolbar
                        </label>
                    </Section>

                    <Section id="tour" title="Guided tour">
                        <p style={help}>
                            Replay the first-run walkthrough of the palette, canvas, properties, Run and the
                            web dashboard.
                        </p>
                        <button
                            type="button"
                            style={btn}
                            onClick={() => {
                                onClose();
                                setTimeout(() => window.dispatchEvent(new Event('duckle:start-tour')), 250);
                            }}
                        >
                            Replay guided tour
                        </button>
                    </Section>
                </div>
                <div className="modal-footer" style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
                    <button type="button" style={btn} onClick={onClose}>
                        Close
                    </button>
                    <button type="button" style={primary} onClick={save} disabled={saving || !workspace}>
                        {saving ? <Loader2 size={14} className="spin" /> : saved ? <Check size={14} /> : null}
                        {saved ? 'Saved' : 'Save'}
                    </button>
                </div>
            </div>
        </div>,
        document.body
    );
}
