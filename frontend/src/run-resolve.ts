import type { Node } from '@xyflow/react';
import type { DuckleNodeData } from './pipeline-types';
import type { ContextPayload, RepoItem, RoutinePayload } from './repo-types';

/**
 * Resolve a pipeline's nodes for execution:
 *   1. Inline a referenced SQL routine into Custom-SQL nodes.
 *   2. Substitute `${var}` / `${context.var}` references in field values
 *      with the workspace's context variables.
 *
 * Run on the working nodes right before they're sent to the engine, so
 * the canvas keeps the un-substituted, editable values.
 */
export function buildContextVars(repo: RepoItem[]): Record<string, string> {
    const out: Record<string, string> = {};
    for (const item of repo) {
        if (item.type !== 'context') continue;
        const payload = item.payload as ContextPayload | undefined;
        if (!payload?.variables) continue;
        for (const v of payload.variables) {
            // Both the bare key and a context-namespaced key resolve.
            out[v.key] = v.value;
            out[`${item.name}.${v.key}`] = v.value;
        }
    }
    return out;
}

function substituteString(value: string, vars: Record<string, string>): string {
    return value.replace(/\$\{([^}]+)\}/g, (match, expr) => {
        const key = String(expr).trim();
        return Object.prototype.hasOwnProperty.call(vars, key) ? vars[key]! : match;
    });
}

function substituteDeep(value: unknown, vars: Record<string, string>): unknown {
    if (typeof value === 'string') return substituteString(value, vars);
    if (Array.isArray(value)) return value.map(v => substituteDeep(v, vars));
    if (value && typeof value === 'object') {
        const out: Record<string, unknown> = {};
        for (const [k, v] of Object.entries(value)) out[k] = substituteDeep(v, vars);
        return out;
    }
    return value;
}

export function resolveForRun(
    nodes: Node<DuckleNodeData>[],
    repo: RepoItem[],
): Node<DuckleNodeData>[] {
    const vars = buildContextVars(repo);
    const sqlRoutines = new Map<string, string>();
    for (const item of repo) {
        if (item.type !== 'routine') continue;
        const payload = item.payload as RoutinePayload | undefined;
        if (payload?.language === 'sql' && payload.code) {
            sqlRoutines.set(item.id, payload.code);
            sqlRoutines.set(item.name, payload.code);
        }
    }
    const hasVars = Object.keys(vars).length > 0;

    return nodes.map(node => {
        const props = { ...(node.data.properties ?? {}) } as Record<string, unknown>;

        // Inline a referenced SQL routine when there's no inline SQL.
        if (node.data.componentId === 'code.sql' || node.data.componentId === 'code.sqltemplate') {
            const ref = typeof props.routineRef === 'string' ? props.routineRef : '';
            const inline = typeof props.sql === 'string' ? props.sql.trim() : '';
            if (ref && !inline && sqlRoutines.has(ref)) {
                props.sql = sqlRoutines.get(ref);
            }
        }

        const resolved = hasVars
            ? (substituteDeep(props, vars) as Record<string, unknown>)
            : props;
        return { ...node, data: { ...node.data, properties: resolved } };
    });
}
