import type { Run } from './api';
import type { ReactNode } from 'react';

export function RunActivity({
  run,
  children,
  onInspect,
}: {
  run: Run;
  children?: ReactNode;
  onInspect?: () => void;
}) {
  const computeEnabled = run.node_scope === 'compute';
  const sources = run.searches.flatMap((item) => item.sources);
  const maxTurns = run.workspace_tools
    ? 16
    : run.image_profile && !run.search_enabled && !run.import_enabled && run.node_scope === 'off'
      ? 2
      : computeEnabled || run.import_enabled
        ? 5
        : run.execution_revision_id
          ? 4
          : 3;
  return (
    <div className="search-activity" aria-label="Tool activity">
      {run.local_model && (
        <p
          className={run.local_model.phase === 'blocked' ? 'run-error' : 'run-stats'}
          role="status"
          aria-label="Model runtime status"
        >
          {
            {
              waiting: 'Model runtime: waiting for its turn.',
              working: 'Model runtime: request active.',
              draining:
                'Model runtime: cleanup pending. The run has ended, but its original request is still in flight.',
              recovering: 'Model runtime: cleanup pending; confirming that inference has stopped.',
              blocked:
                'Model runtime: completion unconfirmed. This resource is blocked until the operator verifies it is idle.',
              settled: 'Model runtime: this request is settled. Model weights may remain loaded.',
            }[run.local_model.phase]
          }
        </p>
      )}
      <details>
        <summary>
          Run details · {run.model_rounds} model turn{run.model_rounds === 1 ? '' : 's'}
          {run.searches.length > 0 && ` · ${run.searches.length} searches`}
          {run.imports.length > 0 && ` · ${run.imports.length} imports`}
          {computeEnabled && ` · ${run.tool_executions.length} computations`}
          {run.image_job && ' · image'}
        </summary>
        {children}
        <p>
          {run.model_rounds} of {maxTurns} model turns permitted
          {run.workspace_tools && ' · workspace files enabled'}
        </p>
        {run.search_enabled && !run.searches.length && (
          <p>Search permitted for this request; no search has been performed.</p>
        )}
        {run.searches.map((item) => (
          <p key={item.id}>
            {item.query} · {item.status}
            {item.elapsed_ms !== null && ` · ${item.elapsed_ms} ms`}
            {item.error && <span className="run-error"> · {item.error}</span>}
          </p>
        ))}
        {run.import_enabled && !run.imports.length && (
          <p>File import permitted; no file has been saved from a URL.</p>
        )}
        {run.imports.map((asset) => (
          <p key={asset.id}>
            Imported {asset.name} · {asset.bytes} bytes · saved in workspace
          </p>
        ))}
        {computeEnabled && !run.tool_executions.length && (
          <p>File analysis permitted; no computation was requested.</p>
        )}
        {computeEnabled &&
          run.tool_executions.map((exec, index) => (
            <p key={exec.id}>
              compute_node #{index + 1} · {exec.result?.status ?? 'No saved result'}
            </p>
          ))}
        {run.execution_revision_id && !run.tool_execution && (
          <p>Saved-script permission granted; no script action has been requested.</p>
        )}
        {run.tool_execution && !computeEnabled && (
          <p>
            {run.tool_execution.action} {run.tool_execution.entry} ·{' '}
            {run.tool_execution.result?.status ?? 'No saved result'}
          </p>
        )}
        {run.image_profile && !run.image_job && (
          <p>
            Image generation permitted with {run.image_profile.label}; no image job was requested.
          </p>
        )}
        {onInspect && (
          <button type="button" onClick={onInspect}>
            Open run details
          </button>
        )}
      </details>
      {sources.length > 0 && (
        <ul aria-label="Search sources">
          {sources.map((source) => (
            <li key={source.id}>
              <a
                href={source.url}
                target="_blank"
                rel="noopener noreferrer"
                referrerPolicy="no-referrer"
              >
                [{source.id}] {source.title}
              </a>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
