import { createContext } from 'react';
import type { Column } from '../../pipeline-types';
import type { ConnectionPayload, RepoItem, RoutinePayload } from '../../repo-types';

export type FieldContextValue = {
    upstreamSchema: Column[];
    nodeSchema: Column[];
    repoItems: RepoItem[];
    onPickConnection?: (payload: ConnectionPayload) => void;
    onPickRoutine?: (payload: RoutinePayload) => void;
};

export const FieldContext = createContext<FieldContextValue>({
    upstreamSchema: [],
    nodeSchema: [],
    repoItems: [],
});
