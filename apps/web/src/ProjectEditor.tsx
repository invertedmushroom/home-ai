import { useEffect, useRef, useState } from 'react';
import { api, HttpError } from './api';
import type { Proposal } from './api';
import { CodeEditor } from './CodeEditor';
import {
  applyChanges,
  applyAssetChanges,
  fileDiff,
  filesSchema,
  projectChanges,
  projectAssetChanges,
  projectSnapshotSchema,
} from './project';
import type { EditorDraft, ProjectFile } from './project';

type Props = {
  workspace: string;
  draft: EditorDraft;
  headId: string | null;
  busy: boolean;
  view: 'source' | 'diff';
  onView: (view: 'source' | 'diff') => void;
  onExport: () => void;
  onDraft: (draft?: EditorDraft) => void;
  onSaved: (proposal: Proposal, path: string) => void;
};

export function ProjectEditor({
  workspace,
  draft,
  headId,
  busy,
  view,
  onView,
  onExport,
  onDraft,
  onSaved,
}: Props) {
  const [newPath, setNewPath] = useState('');
  const compare = view === 'diff';
  const [error, setError] = useState('');
  const [working, setWorking] = useState(false);
  const owner = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const mutating = useRef(false);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      owner.current?.abort();
    };
  }, []);
  const locked = !!draft.request || working;
  const changes = projectChanges(draft.baseFiles, draft.files);
  const assetChanges = projectAssetChanges(draft.baseBindings ?? [], draft.assetBindings ?? []);
  const changeCount = changes.length + assetChanges.length;
  const paths = [...new Set([...draft.files, ...draft.baseFiles].map((file) => file.path))].sort();
  const path = paths.includes(draft.path) ? draft.path : (paths[0] ?? '');
  const selectedPath = useRef(path);
  selectedPath.current = path;
  const file = draft.files.find((item) => item.path === path);
  const before = draft.baseFiles.find((item) => item.path === path);
  const stale = draft.baseId !== headId;
  const comparisonFits = !compare || !!fileDiff(before?.content ?? '', file?.content ?? '');

  function addFile(restored?: ProjectFile) {
    if (locked) return;
    try {
      const added = restored ?? { path: newPath.trim(), content: '' };
      const files = filesSchema.parse([...draft.files, added]);
      projectSnapshotSchema.parse({ files, asset_bindings: draft.assetBindings ?? [] });
      onDraft({ ...draft, files, path: added.path });
      setNewPath('');
      onView('source');
      setError('');
    } catch {
      setError('Use a unique portable path. The project must fit within 64 files and 1 MiB.');
    }
  }

  async function save() {
    if (busy || mutating.current || (!draft.request && (stale || !changeCount))) return;
    let input = draft.request;
    try {
      if (!input) {
        if (!changeCount || changeCount > 64) throw new Error('Invalid project changes.');
        projectSnapshotSchema.parse({
          files: applyChanges(draft.baseFiles, changes),
          asset_bindings: applyAssetChanges(draft.baseBindings ?? [], assetChanges),
        });
        input = {
          id: crypto.randomUUID(),
          base_revision: { id: draft.baseId },
          changes,
          ...(assetChanges.length ? { asset_changes: assetChanges } : {}),
        };
      }
    } catch {
      setError(
        'Changes must fit within 64 portable text files and 1 MiB. Your draft is preserved.',
      );
      return;
    }
    const controller = new AbortController();
    owner.current = controller;
    mutating.current = true;
    setWorking(true);
    setError('');
    onDraft({ ...draft, request: input });
    try {
      const proposal = await api.proposeEdits(workspace, input, controller.signal);
      if (!controller.signal.aborted) onSaved(proposal, selectedPath.current);
    } catch (error) {
      if (!controller.signal.aborted) {
        setError(error instanceof Error ? error.message : 'Could not save this proposal.');
        if (error instanceof HttpError && [400, 403, 404, 409, 413, 422].includes(error.status))
          onDraft({ ...draft, request: undefined });
      }
    } finally {
      if (!controller.signal.aborted && alive.current) setWorking(false);
      mutating.current = false;
    }
  }

  return (
    <section
      className="project-editor"
      aria-label="Editor draft"
      onKeyDownCapture={(event) => {
        if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 's') {
          event.preventDefault();
          event.stopPropagation();
          void save();
        }
      }}
    >
      {stale && (
        <p className="run-error">
          {draft.request
            ? 'The accepted revision changed. Retry checks the captured save without rebasing this draft.'
            : 'The accepted revision changed. Export or discard this draft, then edit the current revision.'}
        </p>
      )}
      {error && (
        <p role="alert" className="run-error">
          {error}
        </p>
      )}
      <div className="artifact-tabs editor-viewbar">
        <button className={!compare ? 'selected' : ''} onClick={() => onView('source')}>
          Source
        </button>
        <button className={compare ? 'selected' : ''} onClick={() => onView('diff')}>
          Diff
        </button>
        <div className="editor-primary-actions">
          {stale && !draft.request ? (
            <button onClick={onExport}>Export draft</button>
          ) : (
            <button
              disabled={busy || working || (!draft.request && (!changeCount || stale))}
              onClick={() => void save()}
              title="Save for review (Ctrl/Cmd+S)"
            >
              {working ? 'Saving…' : draft.request ? 'Retry save' : 'Save for review'}
            </button>
          )}
          <button disabled={locked} onClick={() => onDraft(undefined)}>
            Discard draft
          </button>
        </div>
      </div>
      <div className="new-file">
        <input
          aria-label="New file path"
          placeholder="src/example.js"
          value={newPath}
          disabled={locked}
          onChange={(event) => setNewPath(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              addFile();
            }
          }}
        />
        <button disabled={locked || !newPath.trim()} onClick={() => addFile()}>
          Add file
        </button>
        <button
          disabled={locked || (!file && !before)}
          onClick={() => {
            if (!file && before) addFile(before);
            else onDraft({ ...draft, files: draft.files.filter((item) => item.path !== path) });
          }}
        >
          {!file && before ? 'Restore file' : 'Delete file'}
        </button>
      </div>
      <div className="file-workspace">
        <nav className="file-tree" aria-label="Draft files">
          {paths.map((name) => (
            <button
              key={name}
              aria-current={name === path ? 'true' : undefined}
              onClick={() => onDraft({ ...draft, path: name })}
            >
              {name}
              {!draft.files.some((file) => file.path === name)
                ? ' · deleted'
                : changes.some((change) => change.path === name)
                  ? ' *'
                  : ''}
            </button>
          ))}
        </nav>
        <div className="code-pane">
          {path && !comparisonFits ? (
            <p className="artifact-note">
              Diff exceeds the display budget. Switch to Source to inspect the full text.
            </p>
          ) : path ? (
            <CodeEditor
              identity={`${workspace}:draft`}
              path={path}
              value={file?.content ?? ''}
              original={compare ? (before?.content ?? '') : undefined}
              readOnly={locked || compare || !file}
              onChange={
                compare || locked || !file
                  ? undefined
                  : (content) =>
                      onDraft({
                        ...draft,
                        files: draft.files.map((item) =>
                          item.path === path ? { ...item, content } : item,
                        ),
                      })
              }
            />
          ) : (
            <p className="artifact-note">Add a file to begin, or save the proposed deletions.</p>
          )}
        </div>
      </div>
      <p className="draft-status" role="status">
        {draft.request ? 'Save unconfirmed' : 'Unsaved draft'} · {changeCount} changed file
        {changeCount === 1 ? '' : 's'}
        {' · '}Save for review keeps a proposal; Apply changes accepts it.
      </p>
      {draft.request && (
        <p className="artifact-note">
          Retry checks the same saved request without creating a duplicate. Editing resumes after
          its outcome is known.
        </p>
      )}
    </section>
  );
}
