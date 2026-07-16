// Export the Duckle component catalog (ids, labels, kinds, summaries, property
// schemas and ports) to a single JSON file the duckle-mcp crate embeds via
// include_str!. The frontend manifest stays the single source of truth; this
// just serializes it. Run via scripts/build-catalog.mjs (esbuild bundles this
// and stubs the Tauri bridge so it runs under plain Node).
//
// Output path comes from the CATALOG_OUT env var set by the runner.

import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { ALL_COMPONENTS } from '../src/workflow-ui/palette-data';
import { getManifest } from '../src/workflow-ui/fields/component-manifests';
import { portsForComponent } from '../src/workflow-ui/fields/manifest-synth';

const components = ALL_COMPONENTS.map((c) => {
    // Explicit manifests override the generic synthesized definition. This is
    // essential for components such as src.query, whose fields and ports do
    // not follow the standard source topology.
    const manifest = getManifest(c.id) ?? null;
    return {
        id: c.id,
        label: c.label,
        kind: c.kind,
        availability: c.availability,
        summary: c.summary ?? '',
        ports: manifest?.ports ?? portsForComponent(c),
        manifest,
    };
});

const catalog = {
    version: '1',
    componentCount: components.length,
    components,
};

const out = process.env.CATALOG_OUT;
if (!out) {
    throw new Error('CATALOG_OUT env var not set');
}
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(catalog, null, 2) + '\n');
// eslint-disable-next-line no-console
console.error(`export-catalog: wrote ${components.length} components to ${out}`);
