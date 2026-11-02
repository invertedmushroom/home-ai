import { useEffect, useRef, useState } from 'react';
import { api, executing, HttpError } from './api';
import type { Execution, ExecutionRequest, Proposal, Revision, WorkerStatus } from './api';

type Props = {
  workspace: string;
  revision?: Revision;
  selectedPath: string;
  observedExecutionIds: string[];
  observeToolRun: boolean;
  onSelectScript: (path: string) => void;
  draftActive?: boolean;
  revisions: Revision[];
  onProposal: (proposal: Proposal) => Promise<void>;
  onFix: (job: Execution) => void;
  onExplain?: (job: Execution) => Promise<void>;
  explainBackend?: string;
};
const errorText = (error: unknown) =>
  error instanceof Error ? error.message : 'Cannot read execution state.';

export function Executions({
  workspace,
  revision,
  selectedPath,
  observedExecutionIds,
  observeToolRun,
  onSelectScript,
  revisions,
  onProposal,
  onFix,
  onExplain,
  explainBackend,
  draftActive = false,
}: Props) {
  const [worker, setWorker] = useState<WorkerStatus>();
  const [jobs, setJobs] = useState<Execution[]>([]);
  const [selected, setSelected] = useState('');
  const [toolsOpen, setToolsOpen] = useState(false);
  const [uncertain, setUncertain] = useState<ExecutionRequest | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const owner = useRef<AbortController | null>(null);
  const mutating = useRef(false);
  const sequence = useRef(0);
  const explanation = useRef<{ id: string; send: (job: Execution) => Promise<void> } | null>(null);
  const active = jobs.find(executing);
  const polling = !!active || !!uncertain || observeToolRun;
  const observed = observedExecutionIds.join(',');
  const files = revision?.files.filter((file) => /\.(js|mjs|cjs)$/.test(file.path)) ?? [];
  const scriptSelected = /\.(js|mjs|cjs)$/.test(selectedPath);
  const selectedEntry = scriptSelected
    ? (files.find((file) => file.path === selectedPath)?.path ?? '')
    : (files[0]?.path ?? '');
  const job = jobs.find((job) => job.id === selected) ?? jobs.at(-1);

  function remember(incoming: Execution[]) {
    setJobs((current) => {
      const merged = new Map(current.map((job) => [job.id, job]));
      for (const job of incoming.filter((item) => item.revision_id !== null)) {
        const previous = merged.get(job.id);
        if (!previous || executing(previous) || !executing(job)) merged.set(job.id, job);
      }
      return [...merged.values()].sort((a, b) => a.created_at_ms - b.created_at_ms);
    });
    setUncertain((request) =>
      request && incoming.some((job) => job.id === request.id) ? null : request,
    );
  }

  useEffect(() => {
    const controller = new AbortController();
    owner.current = controller;
    void api
      .worker(controller.signal)
      .then((value) => {
        if (!controller.signal.aborted) setWorker(value);
      })
      .catch((error) => {
        if (!controller.signal.aborted) setError(errorText(error));
      });
    return () => {
      controller.abort();
      explanation.current = null;
      sequence.current++;
    };
  }, [workspace]);

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function read() {
      const ticket = ++sequence.current;
      try {
        const jobs = await api.executions(workspace, controller.signal);
        if (!controller.signal.aborted && ticket === sequence.current) remember(jobs);
      } catch (error) {
        if (!controller.signal.aborted && ticket === sequence.current) setError(errorText(error));
      } finally {
        if (!controller.signal.aborted && polling) timer = setTimeout(() => void read(), 500);
      }
    }
    void read();
    return () => {
      controller.abort();
      if (timer) clearTimeout(timer);
    };
  }, [workspace, polling, observed]);

  async function refresh() {
    const controller = owner.current;
    if (!controller || controller.signal.aborted) return;
    setError('');
    const ticket = ++sequence.current;
    const results = await Promise.allSettled([
      api.worker(controller.signal),
      api.executions(workspace, controller.signal),
    ]);
    if (controller.signal.aborted) return;
    if (results[0].status === 'fulfilled') setWorker(results[0].value);
    else setError(errorText(results[0].reason));
    if (results[1].status === 'fulfilled' && ticket === sequence.current)
      remember(results[1].value);
    else if (results[1].status === 'rejected') setError(errorText(results[1].reason));
  }

  async function explain(result: Execution, send = onExplain) {
    const controller = owner.current;
    if (!send || mutating.current || !controller || controller.signal.aborted) return;
    mutating.current = true;
    setWorking(true);
    setError('');
    try {
      await send(result);
    } catch (error) {
      if (!controller.signal.aborted) setError(errorText(error));
    } finally {
      if (!controller.signal.aborted) setWorking(false);
      mutating.current = false;
    }
  }

  useEffect(() => {
    const intent = explanation.current;
    if (working || !intent) return;
    const completed = jobs.find((job) => job.id === intent.id && !executing(job));
    if (!completed) return;
    explanation.current = null;
    if (!['cancelled', 'interrupted'].includes(completed.status))
      void explain(completed, intent.send);
  }, [jobs, working]);

  async function start(action: 'check' | 'run', explainWhenDone = false) {
    const controller = owner.current;
    if (mutating.current || !controller || controller.signal.aborted) return;
    const input =
      uncertain ??
      (revision && selectedEntry
        ? { id: crypto.randomUUID(), revision_id: revision.id, entry: selectedEntry, action }
        : null);
    if (!input) return;
    if (!uncertain)
      explanation.current = explainWhenDone && onExplain ? { id: input.id, send: onExplain } : null;
    mutating.current = true;
    setWorking(true);
    setUncertain(input);
    setError('');
    try {
      const receipt = await api.execute(workspace, input, controller.signal);
      if (!controller.signal.aborted) {
        remember([receipt]);
        setSelected(receipt.id);
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        setError(errorText(error));
        if (error instanceof HttpError && [400, 404, 409, 413, 422, 503].includes(error.status)) {
          setUncertain(null);
          explanation.current = null;
        }
      }
    } finally {
      if (!controller.signal.aborted) setWorking(false);
      mutating.current = false;
    }
  }

  async function stop() {
    const controller = owner.current;
    if (!active || mutating.current || !controller || controller.signal.aborted) return;
    explanation.current = null;
    mutating.current = true;
    setWorking(true);
    setError('');
    try {
      const receipt = await api.cancelExecution(active.id, controller.signal);
      if (!controller.signal.aborted) remember([receipt]);
    } catch (error) {
      if (!controller.signal.aborted) setError(errorText(error));
    } finally {
      if (!controller.signal.aborted) setWorking(false);
      mutating.current = false;
    }
  }

  async function reviewOutputs() {
    const controller = owner.current;
    if (!job || mutating.current || !controller || controller.signal.aborted) return;
    mutating.current = true;
    setWorking(true);
    setError('');
    try {
      const proposal = await api.proposeOutputs(job, controller.signal);
      if (!controller.signal.aborted) await onProposal(proposal);
    } catch (error) {
      if (!controller.signal.aborted) setError(errorText(error));
    } finally {
      if (!controller.signal.aborted) setWorking(false);
      mutating.current = false;
    }
  }

  const controls = (
    <>
      <div className="execution-controls">
        {scriptSelected ? (
          <strong className="execution-entry">{selectedPath}</strong>
        ) : (
          <select
            aria-label="Execution entry"
            value={selectedEntry}
            aria-describedby="execution-input-note"
            disabled={working || !!uncertain || !files.length}
            onChange={(event) => onSelectScript(event.target.value)}
          >
            {!files.length && (
              <option value="">
                {revision
                  ? 'No JavaScript files in this revision'
                  : 'Choose a saved revision first'}
              </option>
            )}
            {files.map((file) => (
              <option key={file.path} value={file.path}>
                {file.path}
              </option>
            ))}
          </select>
        )}
        {uncertain ? (
          <button disabled={working} onClick={() => void start(uncertain.action)}>
            Retry same execution
          </button>
        ) : (
          <>
            <button
              disabled={working || !!active || !worker?.ready || !selectedEntry}
              onClick={() => void start('run')}
            >
              Run script
            </button>
            <button
              disabled={working || !!active || !worker?.ready || !selectedEntry}
              onClick={() => void start('check')}
            >
              Check syntax
            </button>
            <button
              disabled={working || !!active || !worker?.ready || !selectedEntry || !onExplain}
              title={
                !onExplain
                  ? explainBackend
                    ? 'Wait for the current model request to finish.'
                    : 'Select a real model backend to explain results.'
                  : explainBackend
                    ? `Run this script, then send its saved logs to ${explainBackend}.`
                    : 'Select a real model backend to explain results.'
              }
              onClick={() => void start('run', true)}
            >
              Run and explain
            </button>
          </>
        )}
        {active && (
          <button disabled={working || active.status === 'cancelling'} onClick={() => void stop()}>
            {active.status === 'cancelling' ? 'Cleaning up…' : 'Stop execution'}
          </button>
        )}
      </div>
      <p className="artifact-note" id="execution-input-note">
        {!revision
          ? draftActive
            ? 'Save your editor draft as a proposal and apply it before running. Choose Current files to run accepted files.'
            : `Node runs saved JavaScript files. Apply pending changes first.${revisions.length ? ' Or choose Current files above.' : ''}`
          : !files.length
            ? `Revision ${revision.number} has no .js, .mjs or .cjs files. Use Run preview to open its HTML app.`
            : !selectedEntry
              ? `This file is not present in saved revision ${revision.number}. Choose a saved script to run.`
              : `Runs ${selectedEntry} from saved revision ${revision.number} in an isolated Node environment. Saved project text files are available; binary image bindings are not mounted. Write generated files to /output or ./output.`}
      </p>
      {explainBackend && (
        <p className="artifact-note">
          Run and explain sends saved logs to the model selected when you start. Your unsent message
          stays in the composer.
        </p>
      )}
      {uncertain && !working && (
        <p className="artifact-note">
          Checking whether {uncertain.entry} was accepted. Retry uses the same saved revision,
          script and action, even if the current selection changes.
        </p>
      )}
      <div className="execution-worker">
        <span>{worker?.message ?? 'Checking the execution worker…'}</span>
        <button onClick={() => void refresh()}>Refresh worker</button>
      </div>
    </>
  );

  return (
    <section className="executions" aria-label="Node execution">
      {scriptSelected ? (
        controls
      ) : (
        <details
          className="execution-tools"
          open={toolsOpen}
          onToggle={(event) => setToolsOpen(event.currentTarget.open)}
        >
          <summary>Run JavaScript</summary>
          {controls}
        </details>
      )}
      {error && (
        <p role="alert" className="run-error">
          {error}
        </p>
      )}
      {job && (
        <section className="execution-card" aria-label="Execution results">
          <div className="execution-card-heading">
            <strong>{job.entry}</strong>
            <span className="execution-status">{job.status}</span>
          </div>
          <p className="run-stats">
            {job.action === 'check' ? 'Syntax check' : 'Script run'} · revision{' '}
            {revisions.find((revision) => revision.id === job.revision_id)?.number ??
              job.revision_id}{' '}
            {job.exit_code !== null && ` · exit ${job.exit_code}`}
            {job.signal && ` · ${job.signal}`}
            {job.truncated && ' · output truncated'}
          </p>
          {revisions.at(-1) && job.revision_id !== revisions.at(-1)?.id && (
            <p className="artifact-note">
              This result is from an older revision. New proposals target the current accepted
              files.
            </p>
          )}
          <button disabled={working || executing(job)} onClick={() => onFix(job)}>
            Ask AI to fix
          </button>
          <button
            disabled={working || executing(job) || !onExplain}
            onClick={() => void explain(job)}
            title={
              !onExplain
                ? explainBackend
                  ? 'Wait for the current model request to finish.'
                  : 'Select a real model backend to explain results.'
                : explainBackend
                  ? `Send these saved logs to ${explainBackend}. The script will not run again.`
                  : 'Select a real model backend to explain results.'
            }
          >
            Explain result
          </button>
          {job.error && <p className="run-error">{job.error}</p>}
          {job.status === 'succeeded' && job.action === 'check' && (
            <p className="artifact-note">
              Syntax check passed. Run script executes the script and captures generated files.
            </p>
          )}
          {job.status === 'succeeded' && job.action === 'run' && !job.outputs.length && (
            <p className="run-error" role="status">
              No output files were captured. A completion message in stdout does not save project
              files. Write files to /output or ./output, then run again. Jobs from an older worker
              may need its image rebuilt. Other temporary files are discarded.
            </p>
          )}
          {job.outputs.length > 0 && (
            <div className="execution-outputs">
              <h4>Generated files</h4>
              <p className="artifact-note">
                {job.outputs.length} output file{job.outputs.length === 1 ? '' : 's'} captured
                {' · Review and apply these files to add them to Project files.'}
              </p>
              <ul>
                {job.outputs.map((file) => (
                  <li key={file.path}>
                    {file.path} · {file.bytes} bytes
                  </li>
                ))}
              </ul>
              <button disabled={working} onClick={() => void reviewOutputs()}>
                Review output changes
              </button>
            </div>
          )}
          <details>
            <summary>Execution logs</summary>
            <p>stdout</p>
            <pre className="source" aria-label="Execution stdout">
              {job.stdout || '(no stdout)'}
            </pre>
            <p>stderr</p>
            <pre className="source" aria-label="Execution stderr">
              {job.stderr || '(no stderr)'}
            </pre>
          </details>
          <details>
            <summary>Execution history</summary>
            <select
              aria-label="Execution history"
              value={job.id}
              onChange={(event) => setSelected(event.target.value)}
            >
              {jobs.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.action} {item.entry} · {item.status}
                </option>
              ))}
            </select>
          </details>
        </section>
      )}
    </section>
  );
}
