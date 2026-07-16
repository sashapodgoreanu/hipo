import { useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Check, ChevronDown, X } from 'lucide-react';
import { FieldContext } from './workflow-ui/fields/FieldContext';
import type { RepoItem } from './repo-types';

type Props = {
    value: unknown;
    onChange: (ids: string[]) => void;
};

type DataSourceOption = {
    item: RepoItem;
    alias: string;
};

/** Reference-only selector: it stores stable ids and never copies connection payloads. */
export default function DataSourceRefField({ value, onChange }: Props) {
    const { repoItems } = useContext(FieldContext);
    const selected = Array.isArray(value) ? value.map(String) : [];
    const dataSources = useMemo<DataSourceOption[]>(
        () => repoItems
            .filter(item => item.type === 'data_source')
            .map(item => ({
                item,
                alias: (item.payload as { sqlAlias?: string } | undefined)?.sqlAlias ?? item.name,
            })),
        [repoItems],
    );
    const [query, setQuery] = useState('');
    const [open, setOpen] = useState(false);
    const rootRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        const close = (event: MouseEvent) => {
            if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
        };
        document.addEventListener('mousedown', close);
        return () => document.removeEventListener('mousedown', close);
    }, []);

    const selectedSet = new Set(selected);
    const selectedItems = selected
        .map(id => dataSources.find(option => option.item.id === id))
        .filter((option): option is DataSourceOption => Boolean(option));
    const normalizedQuery = query.trim().toLocaleLowerCase();
    const filtered = dataSources.filter(({ item }) => {
        if (selectedSet.has(item.id)) return false;
        if (!normalizedQuery) return true;
        return item.name.toLocaleLowerCase().includes(normalizedQuery);
    });

    const toggle = (id: string) => {
        const next = selectedSet.has(id)
            ? selected.filter(valueId => valueId !== id)
            : [...selected, id];
        onChange(next);
        setQuery('');
        setOpen(true);
        inputRef.current?.focus();
    };

    const remove = (id: string) => onChange(selected.filter(valueId => valueId !== id));

    return (
        <div className="data-source-ref-field" ref={rootRef}>
            <div
                className={'data-source-ref-control' + (open ? ' is-open' : '')}
                onClick={() => {
                    setOpen(true);
                    inputRef.current?.focus();
                }}
            >
                <div className="data-source-ref-chips">
                    {selectedItems.map(({ item }) => (
                        <span className="data-source-ref-chip" key={item.id}>
                            {item.name}
                            <button
                                type="button"
                                onClick={event => {
                                    event.stopPropagation();
                                    remove(item.id);
                                }}
                                aria-label={`Remove ${item.name}`}
                            >
                                <X size={12} />
                            </button>
                        </span>
                    ))}
                    <input
                        ref={inputRef}
                        className="data-source-ref-input"
                        value={query}
                        onChange={event => {
                            setQuery(event.target.value);
                            setOpen(true);
                        }}
                        onFocus={() => setOpen(true)}
                        onKeyDown={event => {
                            if (event.key === 'Escape') setOpen(false);
                            else if (event.key === 'Enter' && filtered[0]) {
                                event.preventDefault();
                                toggle(filtered[0].item.id);
                            } else if (event.key === 'Backspace' && !query && selected.length > 0) {
                                remove(selected[selected.length - 1]);
                            }
                        }}
                        placeholder={selected.length ? 'Search another data source…' : 'Search by data source name…'}
                        aria-label="Search data sources by name"
                    />
                </div>
                <ChevronDown size={14} className="data-source-ref-chevron" aria-hidden="true" />
            </div>
            {open ? (
                <div className="data-source-ref-menu" role="listbox" aria-label="Data sources">
                    {filtered.length > 0 ? filtered.map(({ item, alias }) => (
                        <button
                            type="button"
                            className="data-source-ref-option"
                            key={item.id}
                            onClick={() => toggle(item.id)}
                            role="option"
                            aria-selected={false}
                        >
                            <span>
                                <span className="data-source-ref-name">{item.name}</span>
                                <span className="data-source-ref-alias">{alias}</span>
                            </span>
                            <Check size={14} aria-hidden="true" />
                        </button>
                    )) : (
                        <div className="data-source-ref-empty">
                            {dataSources.length ? 'No matching data source.' : 'Create a data source first.'}
                        </div>
                    )}
                </div>
            ) : null}
        </div>
    );
}
