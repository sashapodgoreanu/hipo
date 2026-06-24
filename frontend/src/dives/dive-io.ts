// Persistence for dives: they are workspace repo items of type "dive", stored
// as <workspace>/dives/<id>.json and auto-hydrated on load like connections.
// These are thin typed wrappers over the existing per-item payload machinery.

import { saveItemPayload, deleteItemPayload } from '../workspace';
import { parseDive, type Dive } from './dive-types';

export async function saveDive(workspacePath: string, dive: Dive): Promise<boolean> {
    return saveItemPayload(workspacePath, 'dive', dive.id, dive);
}

export async function deleteDive(workspacePath: string, id: string): Promise<void> {
    return deleteItemPayload(workspacePath, 'dive', id);
}

/** Parse a raw payload (e.g. a hydrated repo item) into a Dive, or throw with a
 *  clear message so a bad file fails loudly instead of half-rendering. */
export function loadDive(raw: unknown): Dive {
    const r = parseDive(raw);
    if (!r.ok || !r.dive) throw new Error(r.error || 'Invalid dive file.');
    return r.dive;
}
