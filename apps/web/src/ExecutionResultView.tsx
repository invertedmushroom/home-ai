import type { ExecutionAttachment, Revision } from './api';

type Result = Omit<ExecutionAttachment, 'status' | 'logs_shortened'> & {
  status: string;
  logs_shortened?: boolean;
};

export function ExecutionResultView({
  result,
  revisions,
  included = false,
}: {
  result: Result;
  revisions: Revision[];
  included?: boolean;
}) {
  const revision = revisions.find((item) => item.id === result.revision_id);
  return (
    <div
      className="execution-result"
      aria-label={included ? 'Included execution result' : 'Execution result'}
    >
      <strong>
        {result.action === 'check' ? 'Syntax check' : 'Node run'} · {result.entry} · {result.status}
      </strong>
      <p>
        {result.revision_id
          ? `Revision ${revision?.number ?? result.revision_id.slice(0, 8)}`
          : 'Temporary script'}{' '}
        · {result.id.slice(0, 8)}
        {result.exit_code !== null && ` · exit ${result.exit_code}`}
        {result.signal && ` · ${result.signal}`}
      </p>
      {revision && revisions.at(-1)?.id !== revision.id && (
        <p className="result-note">
          This result is from an older revision. New proposals target the current accepted files.
        </p>
      )}
      {result.error && <p className="run-error">{result.error}</p>}
      {(result.truncated || result.logs_shortened) && (
        <p className="result-note">
          {result.truncated && 'Worker output was truncated. '}
          {result.logs_shortened && 'Attachment keeps the last 8 KiB of each stream.'}
        </p>
      )}
      <details>
        <summary>Execution logs</summary>
        <p>stdout</p>
        <pre>{result.stdout || '(no stdout)'}</pre>
        <p>stderr</p>
        <pre>{result.stderr || '(no stderr)'}</pre>
      </details>
      {included && (
        <p className="result-note">This observation was included when the request was sent.</p>
      )}
    </div>
  );
}
