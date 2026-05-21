import { Channel, invoke } from '@tauri-apps/api/core';
import { isTauri } from './tauri-dialog';
import type { Column } from './pipeline-types';
import type { Edge, Node } from '@xyflow/react';
import type { DuckleNodeData } from './pipeline-types';

type AutodetectPayload = {
    columns: Column[];
    sampleRows: Record<string, unknown>[];
};

/**
 * Call into the Rust `autodetect_schema` Tauri command when running
 * under Tauri. Returns `null` in browser mode or on failure, so the
 * caller can fall back to a mock.
 */
export async function tauriAutodetect(
    format: string,
    options: Record<string, unknown>,
): Promise<AutodetectPayload | null> {
    if (!isTauri()) return null;
    try {
        return await invoke<AutodetectPayload>('autodetect_schema', { format, options });
    } catch (err) {
        console.warn('Tauri autodetect failed for ' + format, err);
        return null;
    }
}

// ---- Pipeline execution ------------------------------------------------

export type NodeRunStatus = {
    status: 'ok' | 'error' | 'running';
    kind?: 'view' | 'sink';
    rows?: number;
    duration_ms?: number;
    error?: string;
};

export type NodePreview = {
    node_id: string;
    columns: Column[];
    rows: Record<string, unknown>[];
};

export type RunResult = {
    status: 'ok' | 'error' | 'cancelled';
    duration_ms: number;
    nodes: Record<string, NodeRunStatus>;
    preview: NodePreview[];
    error?: string;
};

export type PipelineEvent =
    | { type: 'started'; total_stages: number }
    | { type: 'stage_started'; node_id: string; label: string; kind: 'view' | 'sink' }
    | {
          type: 'stage_finished';
          node_id: string;
          kind: 'view' | 'sink';
          status: 'ok' | 'error';
          rows?: number;
          duration_ms: number;
          error?: string;
      }
    | { type: 'cancelled' }
    | { type: 'finished'; status: 'ok' | 'error' | 'cancelled'; duration_ms: number };

export async function runPipeline(
    nodes: Node<DuckleNodeData>[],
    edges: Edge[],
    onEvent?: (evt: PipelineEvent) => void,
): Promise<RunResult | null> {
    if (!isTauri()) return null;
    const channel = new Channel<PipelineEvent>();
    if (onEvent) channel.onmessage = onEvent;
    try {
        return await invoke<RunResult>('run_pipeline', {
            pipeline: { nodes, edges },
            onEvent: channel,
        });
    } catch (err) {
        console.error('runPipeline failed', err);
        return {
            status: 'error',
            duration_ms: 0,
            nodes: {},
            preview: [],
            error: String(err),
        };
    }
}

export async function runPipelinePartial(
    nodes: Node<DuckleNodeData>[],
    edges: Edge[],
    targetNodeId: string,
    onEvent?: (evt: PipelineEvent) => void,
): Promise<RunResult | null> {
    if (!isTauri()) return null;
    const channel = new Channel<PipelineEvent>();
    if (onEvent) channel.onmessage = onEvent;
    try {
        return await invoke<RunResult>('run_pipeline_partial', {
            pipeline: { nodes, edges },
            targetNodeId,
            onEvent: channel,
        });
    } catch (err) {
        console.error('runPipelinePartial failed', err);
        return {
            status: 'error',
            duration_ms: 0,
            nodes: {},
            preview: [],
            error: String(err),
        };
    }
}

export async function cancelPipeline(): Promise<void> {
    if (!isTauri()) return;
    try {
        await invoke('cancel_pipeline');
    } catch (err) {
        console.warn('cancelPipeline failed', err);
    }
}

export type StageSql = {
    node_id: string;
    label: string;
    kind: 'view' | 'sink';
    sql: string;
};

export async function compilePipelineSql(
    nodes: Node<DuckleNodeData>[],
    edges: Edge[],
): Promise<StageSql[] | null> {
    if (!isTauri()) return null;
    try {
        return await invoke<StageSql[]>('compile_pipeline', {
            pipeline: { nodes, edges },
        });
    } catch (err) {
        console.warn('compilePipelineSql failed', err);
        return null;
    }
}
