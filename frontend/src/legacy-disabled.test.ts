import { describe, expect, it } from 'vitest';
import {
    legacyComponentDisplay,
    legacyWorkspaceEngineDiagnostic,
} from './legacy-disabled';
import type { ComponentDef } from './workflow-ui/palette-data';

const dbtComponent: ComponentDef = {
    id: 'xf.dbt',
    label: 'dbt',
    kind: 'transform',
    availability: 'available',
    summary: 'Old persisted dbt component',
};

describe('legacy disabled compatibility diagnostics', () => {
    it('keeps a SlothDB workspace readable but returns an explicit no-fallback diagnostic', () => {
        expect(legacyWorkspaceEngineDiagnostic('slothdb')).toContain('engine_disabled');
        expect(legacyWorkspaceEngineDiagnostic('slothdb')).toContain('SlothDB');
        expect(legacyWorkspaceEngineDiagnostic('duckdb')).toBeNull();
        expect(legacyWorkspaceEngineDiagnostic(undefined)).toBeNull();
    });

    it('renders xf.dbt as disabled without mutating the persisted component definition', () => {
        const displayed = legacyComponentDisplay(dbtComponent);

        expect(displayed).not.toBe(dbtComponent);
        expect(displayed.availability).toBe('planned');
        expect(displayed.summary).toContain('Temporarily disabled');
        expect(displayed.alternateHint).toContain('no fallback');
        expect(dbtComponent.availability).toBe('available');
        expect(dbtComponent.summary).toBe('Old persisted dbt component');
    });

    it('leaves ordinary components unchanged', () => {
        const csv: ComponentDef = {
            id: 'src.csv',
            label: 'CSV',
            kind: 'source',
            availability: 'available',
        };
        expect(legacyComponentDisplay(csv)).toBe(csv);
    });
});
