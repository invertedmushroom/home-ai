import { useEffect, useRef, useState } from 'react';
import { api, pending } from './api';
import type { Asset, Proposal, Run } from './api';

type Props = {
  workspaceId: string;
  run: Run;
  baseRevisionId: string | null;
  busy: boolean;
  onSettlement: (proposal: Proposal) => void;
  refresh: () => Promise<unknown>;
};

function downloadBlob(content: string, name: string, type: string) {
  const url = URL.createObjectURL(new Blob([content], { type }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function isSvgAsset(asset: Asset) {
  return asset.media_type === 'image/svg+xml' || asset.name.toLowerCase().endsWith('.svg');
}

export function ComputeResultCard({
  workspaceId,
  run,
  baseRevisionId,
  busy,
  onSettlement,
  refresh,
}: Props) {
  const executions = run.tool_executions;
  const latest = executions.at(-1);
  const outputMap = new Map<string, Asset>();
  for (const exec of executions) {
    for (const asset of exec.result?.output_assets ?? []) {
      outputMap.set(asset.id, asset);
    }
  }
  const outputAssets = [...outputMap.values()];
  const automatic = run.workspace_tools && (pending(run) || run.status === 'succeeded');

  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [svgContents, setSvgContents] = useState<Record<string, string>>({});
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    const validIds = new Set(outputAssets.map((asset) => asset.id));
    setSelectedIds((current) => {
      const filtered = current.filter((id) => validIds.has(id));
      if (filtered.length > 0) return filtered;
      return outputAssets.map((asset) => asset.id);
    });
  }, [outputAssets.map((asset) => asset.id).join(',')]);

  useEffect(() => {
    const controller = new AbortController();
    for (const asset of outputAssets) {
      if (!isSvgAsset(asset) || svgContents[asset.id] !== undefined) continue;
      void api
        .asset(asset.id, controller.signal)
        .then((detail) => {
          if (!controller.signal.aborted && alive.current) {
            setSvgContents((current) => ({ ...current, [asset.id]: detail.content }));
          }
        })
        .catch(() => {
          // Ignore preview load errors; download or retry will surface if needed.
        });
    }
    return () => controller.abort();
  }, [outputAssets.map((asset) => asset.id).join(',')]);

  if (!latest) return null;

  const status = latest.result?.status ?? (pending(run) ? 'running' : 'incomplete');
  const attemptSummary =
    executions.length > 1
      ? `Call ${executions.length} of ${executions.length} ran after call 1 (${executions[0]?.result?.status ?? 'failed'})`
      : 'Call 1 code ran';

  async function handleDownload(asset: Asset) {
    setError('');
    try {
      const cached = svgContents[asset.id];
      const content = cached ?? (await api.asset(asset.id)).content;
      downloadBlob(content, asset.name, asset.media_type);
    } catch (err) {
      if (alive.current) {
        setError(err instanceof Error ? err.message : 'Cannot download asset.');
      }
    }
  }

  async function handleReviewInProject() {
    if (!selectedIds.length || working || busy) return;
    setWorking(true);
    setError('');
    try {
      const proposal = await api.proposeAssets(workspaceId, {
        base_revision: { id: baseRevisionId },
        asset_ids: selectedIds,
      });
      if (!alive.current) return;
      onSettlement(proposal);
      await refresh();
    } catch (err) {
      if (alive.current) {
        setError(err instanceof Error ? err.message : 'Cannot create project proposal.');
      }
    } finally {
      if (alive.current) setWorking(false);
    }
  }

  function toggleSelected(id: string, checked: boolean) {
    setSelectedIds((current) =>
      checked ? [...new Set([...current, id])] : current.filter((item) => item !== id),
    );
  }

  return (
    <section
      className="execution-result compute-result-card"
      aria-label="Temporary computation result"
    >
      <div className="execution-card-heading">
        <strong>Analysis and outputs · {status}</strong>
        <span>{attemptSummary}</span>
      </div>
      {latest.result?.error && <p className="run-error">{latest.result.error}</p>}
      {latest.result && (
        <details className="compute-logs">
          <summary>Execution output</summary>
          <p>stdout</p>
          <pre aria-label="Computation stdout">{latest.result.stdout || '(no stdout)'}</pre>
          {latest.result.stderr && (
            <>
              <p>stderr</p>
              <pre aria-label="Computation stderr">{latest.result.stderr}</pre>
            </>
          )}
        </details>
      )}
      {outputAssets.length > 0 && (
        <div className="execution-outputs">
          <h4>Output files</h4>
          <ul>
            {outputAssets.map((asset) => (
              <li key={asset.id} className="compute-output-item">
                <label>
                  {!automatic && (
                    <input
                      type="checkbox"
                      checked={selectedIds.includes(asset.id)}
                      onChange={(event) => toggleSelected(asset.id, event.target.checked)}
                    />
                  )}
                  {asset.name} · {asset.media_type} · {asset.bytes} bytes
                </label>
                <button type="button" onClick={() => void handleDownload(asset)}>
                  Download {asset.name}
                </button>
                {isSvgAsset(asset) && svgContents[asset.id] !== undefined && (
                  <div className="compute-svg-wrap">
                    <img
                      className="compute-svg-preview"
                      alt={`Preview of ${asset.name}`}
                      src={`data:image/svg+xml;utf8,${encodeURIComponent(svgContents[asset.id])}`}
                    />
                  </div>
                )}
              </li>
            ))}
          </ul>
          {automatic ? (
            <p>
              {run.status === 'succeeded'
                ? 'Outputs saved automatically with this reply.'
                : 'Outputs will save to project files when this reply succeeds.'}
            </p>
          ) : (
            <button
              type="button"
              disabled={busy || working || selectedIds.length === 0}
              onClick={() => void handleReviewInProject()}
            >
              {working ? 'Creating proposal…' : 'Review selected files in project'}
            </button>
          )}
        </div>
      )}
      {error && (
        <p className="run-error" role="alert">
          {error}
        </p>
      )}
      <details>
        <summary>Submitted code and execution details</summary>
        {executions.map((exec, index) => (
          <div key={exec.id} className="compute-call-detail">
            <p>
              <strong>
                Call {index + 1} · {exec.entry} · {exec.result?.status ?? 'No saved result'}
              </strong>
              {index === executions.length - 1 && ' · Active result'}
              {exec.result?.exit_code !== null &&
                exec.result?.exit_code !== undefined &&
                ` · exit ${exec.result.exit_code}`}
              {exec.result?.signal && ` · ${exec.result.signal}`}
            </p>
            <p className="result-note">
              Inputs:{' '}
              {exec.input_assets.length
                ? exec.input_assets.map((item) => `${item.name} (${item.path})`).join(', ')
                : 'none'}
            </p>
            <pre aria-label={`Submitted code ${index + 1}`}>{exec.code ?? ''}</pre>
            {executions.length > 1 && exec.result && (
              <>
                <p>Call {index + 1} stdout</p>
                <pre>{exec.result.stdout || '(no stdout)'}</pre>
                <p>Call {index + 1} stderr</p>
                <pre>{exec.result.stderr || '(no stderr)'}</pre>
              </>
            )}
          </div>
        ))}
      </details>
    </section>
  );
}
