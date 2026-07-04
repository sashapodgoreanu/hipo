import { useContext } from 'react';
import { X } from 'lucide-react';
import { FieldContext } from './FieldContext';
import { CAST_TYPES, type Cast } from './types';

// #144: multi-column Cast / Convert editor. One row per column -> type mapping,
// so a single Cast node can convert many columns instead of chaining one node
// per column. Mirrors AggregationsField's row model and reuses its grid styles.

type Props = {
    value: Cast[] | undefined;
    onChange: (v: Cast[]) => void;
};

const isDateLike = (t: string) => t === 'date' || t === 'timestamp';

export function CastsField({ value, onChange }: Props) {
    const { upstreamSchema } = useContext(FieldContext);
    const casts = value ?? [];

    const add = () => {
        const taken = new Set(casts.map(c => c.column));
        const next = upstreamSchema.find(c => !taken.has(c.name));
        onChange([...casts, { column: next?.name ?? '', targetType: 'string' }]);
    };

    const update = (i: number, patch: Partial<Cast>) => {
        onChange(casts.map((c, idx) => (idx === i ? { ...c, ...patch } : c)));
    };

    const remove = (i: number) => {
        onChange(casts.filter((_, idx) => idx !== i));
    };

    return (
        <div className="field-aggregations">
            <div className="field-agg-toolbar">
                <span className="field-agg-count">
                    {casts.length} column{casts.length === 1 ? '' : 's'}
                </span>
                <button type="button" className="schema-add" onClick={add}>
                    + Add column
                </button>
            </div>
            {casts.length === 0 ? (
                <div className="field-agg-empty">
                    No conversions defined. Click <b>+ Add column</b> to convert one or more
                    columns to a new type in this single node.
                </div>
            ) : (
                <div className="field-agg-table">
                    <div className="field-agg-row field-agg-header">
                        <div>Column</div>
                        <div>Target type</div>
                        <div>Format</div>
                        <div />
                    </div>
                    {casts.map((c, i) => (
                        <div className="field-agg-row" key={i}>
                            <select
                                className="schema-input"
                                value={c.column}
                                onChange={e => update(i, { column: e.target.value })}
                            >
                                <option value="">- column -</option>
                                {upstreamSchema.map(col => (
                                    <option key={col.name} value={col.name}>
                                        {col.name}
                                    </option>
                                ))}
                                {c.column && !upstreamSchema.some(col => col.name === c.column) ? (
                                    <option value={c.column}>{c.column}  (not in input)</option>
                                ) : null}
                            </select>
                            <select
                                className="schema-input"
                                value={c.targetType}
                                onChange={e => update(i, { targetType: e.target.value })}
                            >
                                {CAST_TYPES.map(t => (
                                    <option key={t.value} value={t.value}>
                                        {t.label}
                                    </option>
                                ))}
                            </select>
                            <input
                                type="text"
                                className="schema-input"
                                value={c.format ?? ''}
                                onChange={e => update(i, { format: e.target.value })}
                                disabled={!isDateLike(c.targetType)}
                                placeholder={isDateLike(c.targetType) ? '%d/%m/%Y' : ''}
                                title="strptime format for parsing strings into a date/timestamp. Only used for date/timestamp targets; blank = ISO auto-detect."
                                spellCheck={false}
                            />
                            <button
                                type="button"
                                className="schema-remove"
                                onClick={() => remove(i)}
                                aria-label="Remove"
                            >
                                <X size={12} />
                            </button>
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}
