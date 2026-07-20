import type { ComponentDef } from './workflow-ui/palette-data';

export const SLOTHDB_DISABLED_DIAGNOSTIC =
    'engine_disabled: SlothDB is temporarily disabled during the sidecar runner migration; no fallback engine will be selected.';

export const XFD_BT_DISABLED_DIAGNOSTIC =
    'Temporarily disabled during the sidecar runner migration.';

/**
 * Persisted workspace metadata remains readable. This helper only reports the
 * execution diagnostic; it never rewrites the selected engine to DuckDB.
 */
export function legacyWorkspaceEngineDiagnostic(engine: string | undefined): string | null {
    return engine?.trim().toLowerCase() === 'slothdb' ? SLOTHDB_DISABLED_DIAGNOSTIC : null;
}

/**
 * Return the palette representation of a component without mutating the
 * persisted definition loaded from a workspace. Enabled legacy xf.dbt nodes are
 * rejected by the Rust planner as well; this prevents authoring new ones.
 */
export function legacyComponentDisplay(component: ComponentDef): ComponentDef {
    if (component.id !== 'xf.dbt') return component;
    return {
        ...component,
        availability: 'planned',
        summary: XFD_BT_DISABLED_DIAGNOSTIC,
        alternateHint: 'Existing nodes remain readable but execution fails explicitly; no fallback is attempted.',
    };
}
