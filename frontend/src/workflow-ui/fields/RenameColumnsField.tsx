import { useContext } from 'react';
import { FieldContext } from './FieldContext';

// #159: column-aware Rename editor. Instead of typing Old -> New key/value
// pairs by hand (and copying names out of the Schema tab), list every upstream
// column with a checkbox + an editable "new name" box, like Drop Columns.
// Checking a column adds a mapping entry; editing sets the new name. Only
// checked columns are emitted, so the stored value stays the same
// `mapping: [{ key: oldName, value: newName }]` shape the engine already reads.

type Pair = { key: string; value: string };

type Props = {
    // Accept unknown so an externally-edited / AI-generated pipeline with a
    // non-conforming shape ({old:new} object instead of a [{key,value}] array)
    // is coerced rather than crashing the panel (#93).
    value: unknown;
    onChange: (v: Pair[]) => void;
};

/** Coerce any stored value into the [{key,value}] shape the editor expects. */
function toPairs(value: unknown): Pair[] {
    if (Array.isArray(value)) {
        return value
            .filter((p): p is Record<string, unknown> => Boolean(p) && typeof p === 'object')
            .map(p => ({ key: String(p.key ?? ''), value: String(p.value ?? '') }));
    }
    if (value && typeof value === 'object') {
        return Object.entries(value as Record<string, unknown>).map(([k, v]) => ({
            key: k,
            value: v == null ? '' : String(v),
        }));
    }
    return [];
}

export function RenameColumnsField({ value, onChange }: Props) {
    const { upstreamSchema } = useContext(FieldContext);
    const pairs = toPairs(value);
    const byOld = new Map(pairs.map(p => [p.key, p.value]));
    const upstreamNames = new Set(upstreamSchema.map(c => c.name));
    // Mappings whose source column is no longer upstream (the input changed).
    // Render them flagged so the user can see and remove them, matching the
    // Drop Columns "not in input" behaviour.
    const stale = pairs.filter(p => !upstreamNames.has(p.key));

    // Replace/insert/drop the entry for one source column (kept keyed by old
    // name; render order follows upstreamSchema so the list never jumps).
    const setPair = (old: string, next: Pair | null) => {
        const others = pairs.filter(p => p.key !== old);
        onChange(next ? [...others, next] : others);
    };
    const toggle = (name: string) => {
        if (byOld.has(name)) setPair(name, null);
        else setPair(name, { key: name, value: name });
    };

    if (upstreamSchema.length === 0 && stale.length === 0) {
        return (
            <div className="field-input field-warning">
                No upstream schema. Connect an input to populate this list.
            </div>
        );
    }

    const renamedCount = upstreamSchema.filter(
        c => byOld.has(c.name) && (byOld.get(c.name) ?? '') !== c.name,
    ).length;

    return (
        <div className="field-rename">
            <div className="field-agg-toolbar">
                <span className="field-agg-count">
                    {renamedCount} renamed
                </span>
            </div>
            <div className="field-rename-list">
                {upstreamSchema.map(c => {
                    const enabled = byOld.has(c.name);
                    return (
                        <div key={c.name} className="field-rename-row">
                            <input
                                type="checkbox"
                                checked={enabled}
                                onChange={() => toggle(c.name)}
                                aria-label={`Rename ${c.name}`}
                            />
                            <span className="field-rename-old" title={c.name}>{c.name}</span>
                            <input
                                type="text"
                                className="schema-input field-rename-new"
                                value={enabled ? (byOld.get(c.name) ?? '') : c.name}
                                disabled={!enabled}
                                placeholder={c.name}
                                spellCheck={false}
                                onChange={e => setPair(c.name, { key: c.name, value: e.target.value })}
                            />
                        </div>
                    );
                })}
                {stale.map(p => (
                    <div key={p.key} className="field-rename-row field-rename-stale">
                        <input
                            type="checkbox"
                            checked
                            onChange={() => setPair(p.key, null)}
                            aria-label={`Remove rename ${p.key}`}
                        />
                        <span className="field-rename-old" title={p.key}>{p.key} (not in input)</span>
                        <input
                            type="text"
                            className="schema-input field-rename-new"
                            value={p.value}
                            spellCheck={false}
                            onChange={e => setPair(p.key, { key: p.key, value: e.target.value })}
                        />
                    </div>
                ))}
            </div>
        </div>
    );
}
