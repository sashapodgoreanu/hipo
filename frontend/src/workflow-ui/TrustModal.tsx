// Trust scorecard viewer. Calls the engine's pipeline_trust_report for the
// active pipeline and shows an explainable 0-100 score where every lost point
// is an itemized finding (compile status, structural risks, ungoverned PII).
// Read-only and static (no source reads), so it is fast right in the editor.
// The same scorecard is what the `duckle review`/`trust_report` MCP tool and the
// CLI report, so the editor agrees with the agent-facing surfaces.

import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import type { Edge, Node } from '@xyflow/react';
import type { DuckleNodeData } from '../pipeline-types';
import { pipelineTrustReport, type TrustReport } from '../tauri-bridge';

interface TrustModalProps {
    nodes: Node<DuckleNodeData>[];
    edges: Edge[];
    onClose: () => void;
}

// Grade -> badge class. Brand rule: a good score is maya (no green).
function gradeClass(grade: string): string {
    if (grade === 'A' || grade === 'B') return 'trust-grade-good';
    if (grade === 'C') return 'trust-grade-warn';
    return 'trust-grade-bad';
}

export function TrustModal({ nodes, edges, onClose }: TrustModalProps) {
    const [data, setData] = useState<TrustReport | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);

    useEffect(() => {
        let cancelled = false;
        void (async () => {
            try {
                const r = await pipelineTrustReport(nodes, edges);
                if (!cancelled) {
                    setData(r);
                    setLoading(false);
                }
            } catch (e) {
                if (!cancelled) {
                    setError(e instanceof Error ? e.message : String(e));
                    setLoading(false);
                }
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [nodes, edges]);

    return (
        <div className="dive-modal-backdrop" onClick={onClose}>
            <div className="trust-modal" onClick={(e) => e.stopPropagation()}>
                <div className="lineage-head">
                    <h2 className="lineage-title">Trust score</h2>
                    <button type="button" className="dive-btn" onClick={onClose} aria-label="Close">
                        <X size={16} />
                    </button>
                </div>
                <p className="lineage-sub">
                    An explainable score for this pipeline. Every lost point is a finding below, so
                    you can see exactly what to fix. Static checks only - no data is read.
                </p>

                {loading ? <div className="dive-panel-msg">Scoring pipeline...</div> : null}
                {error ? <div className="dive-panel-msg dive-panel-err">{error}</div> : null}

                {!loading && !error && data ? (
                    <>
                        <div className="trust-scoreline">
                            <div className={`trust-score ${gradeClass(data.grade)}`}>
                                <span className="trust-score-num">{data.score}</span>
                                <span className="trust-score-max">/100</span>
                            </div>
                            <div className={`trust-grade ${gradeClass(data.grade)}`}>{data.grade}</div>
                            <div className="trust-score-meta">
                                <div className="trust-summary">{data.summary}</div>
                                <div className="trust-compile">
                                    {data.compiles ? 'Compiles' : 'Does not compile'}
                                </div>
                            </div>
                        </div>

                        {data.findings.length === 0 ? (
                            <div className="dive-panel-msg trust-clean">
                                No issues found. This pipeline scores a clean {data.score}/100.
                            </div>
                        ) : (
                            <ul className="trust-findings">
                                {data.findings.map((f, i) => (
                                    <li key={i} className={`trust-finding trust-${f.severity}`}>
                                        <span className="trust-dot" aria-hidden="true" />
                                        <span className="trust-finding-body">
                                            <span className="trust-finding-code">{f.code}</span>
                                            <span className="trust-finding-msg">{f.message}</span>
                                        </span>
                                        {f.deduction > 0 ? (
                                            <span className="trust-deduction">-{f.deduction}</span>
                                        ) : null}
                                    </li>
                                ))}
                            </ul>
                        )}
                    </>
                ) : null}
            </div>
        </div>
    );
}
