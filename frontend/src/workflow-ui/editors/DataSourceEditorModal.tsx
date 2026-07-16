import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Database, Save, X } from 'lucide-react';
import type { DataSourcePayload, RepoItem } from '../../repo-types';

type Props = {
    item: RepoItem | null;
    connections: RepoItem[];
    dataSources: RepoItem[];
    onSave: (name: string, payload: DataSourcePayload) => void;
    onCancel: () => void;
};

const ALIAS = /^[A-Za-z_][A-Za-z0-9_]*$/;

export default function DataSourceEditorModal({ item, connections, dataSources, onSave, onCancel }: Props) {
    const initial = (item?.payload as DataSourcePayload | undefined) ?? null;
    const [name, setName] = useState(item?.name ?? '');
    const [sqlAlias, setSqlAlias] = useState(initial?.sqlAlias ?? '');
    const [kind, setKind] = useState<DataSourcePayload['kind']>(initial?.kind ?? 'duckdb');
    const [connectionRef, setConnectionRef] = useState(initial?.connectionRef ?? '');
    const nameRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        setTimeout(() => nameRef.current?.focus(), 30);
        const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onCancel();
        document.addEventListener('keydown', onKey);
        return () => document.removeEventListener('keydown', onKey);
    }, [onCancel]);

    const compatible = connections.filter(c => {
        const payload = c.payload as { kind?: string } | undefined;
        return payload?.kind === kind;
    });
    const duplicateAlias = dataSources.some(d => d.id !== item?.id && ((d.payload as DataSourcePayload | undefined)?.sqlAlias ?? '').toLowerCase() === sqlAlias.trim().toLowerCase());
    const canSave = Boolean(name.trim() && ALIAS.test(sqlAlias.trim()) && !duplicateAlias && connectionRef);

    return createPortal(
        <div className="modal-backdrop" onClick={e => e.target === e.currentTarget && onCancel()}>
            <div className="modal modal-editor">
                <div className="modal-header">
                    <div className="modal-title-row"><Database size={16} className="modal-title-icon" /><div>
                        <div className="modal-title">{item ? 'Edit data source' : 'New data source'}</div>
                        <div className="modal-subtitle">Reusable catalog reference · saved in <code>Data Sources</code></div>
                    </div></div>
                    <button type="button" className="modal-close" onClick={onCancel} aria-label="Close"><X size={16} /></button>
                </div>
                <div className="modal-body">
                    <label className="modal-field"><span className="modal-field-label">Name</span><input ref={nameRef} className="modal-input" value={name} onChange={e => setName(e.target.value)} /></label>
                    <label className="modal-field"><span className="modal-field-label">SQL alias</span><input className="modal-input" value={sqlAlias} onChange={e => setSqlAlias(e.target.value)} placeholder="sales" />{sqlAlias && !ALIAS.test(sqlAlias) ? <small>Use letters, numbers and underscore.</small> : null}{duplicateAlias ? <small>This alias is already used in the workspace.</small> : null}</label>
                    <label className="modal-field"><span className="modal-field-label">Type</span><select className="modal-input modal-select" value={kind} onChange={e => { setKind(e.target.value as DataSourcePayload['kind']); setConnectionRef(''); }}><option value="duckdb">DuckDB</option><option value="postgres">PostgreSQL</option></select></label>
                    <label className="modal-field"><span className="modal-field-label">Connection</span><select className="modal-input modal-select" value={connectionRef} onChange={e => setConnectionRef(e.target.value)}><option value="">Select a compatible connection</option>{compatible.map(c => <option key={c.id} value={c.id}>{c.name}</option>)}</select>{compatible.length === 0 ? <div className="modal-field-hint">Create a {kind === 'postgres' ? 'PostgreSQL' : 'DuckDB'} connection first.</div> : null}</label>
                </div>
                <div className="modal-footer"><button type="button" className="btn btn-secondary" onClick={onCancel}>Cancel</button><button type="button" className="btn btn-primary" disabled={!canSave} onClick={() => onSave(name.trim(), { kind, sqlAlias: sqlAlias.trim(), connectionRef, readOnly: true })}><Save size={13} /> Save</button></div>
            </div>
        </div>,
        document.body,
    );
}
