import { useEffect, useRef, useState } from 'react';
import { api, pending } from './api';
import type { Execution, Proposal, Revision, WorkspaceDetail } from './api';
import {
  applyChanges,
  applyAssetChanges,
  fileDiff,
  sameChanges,
  sameAssetChanges,
  projectChanges,
  projectAssetChanges,
} from './project';
import { projectPreview } from './preview';
import { Executions } from './Executions';
import { CodeEditor } from './CodeEditor';
import { ProjectEditor } from './ProjectEditor';
import { AssetLibrary } from './AssetLibrary';
import { useDownloads } from './useDownloads';
import { WorkspacePane, WorkspacePanel, useWorkspaceLayout } from './WorkspaceLayout';
import type { EditorDraft } from './project';

type Props = {
  detail: WorkspaceDetail | null;
  selection: ProjectSelection;
  onSelection: (selection: ProjectSelection) => void;
  busy: boolean;
  assetsAvailable?: boolean;
  refresh: () => Promise<unknown>;
  onSettlement: (proposal: Proposal, revision?: Revision) => void;
  draft?: EditorDraft;
  onDraft: (workspace: string, draft?: EditorDraft) => void;
  onFix: (job: Execution) => void;
  onExplain?: (job: Execution) => Promise<void>;
  explainBackend?: string;
};
type Preview = {
  id: string;
  source: { kind: 'revision'; id: string; number: number } | { kind: 'proposal'; id: string };
  entryPath: string;
  document: string;
};
export type ProjectSelection =
  { kind: 'current' | 'draft' } | { kind: 'revision' | 'proposal'; id: string };
type ViewMode = 'source' | 'diff';

function previewImageUrl(blob: Blob, signal: AbortSignal): Promise<string> {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    const abort = () => reader.abort();
    const cleanup = () => signal.removeEventListener('abort', abort);
    reader.onload = () => {
      cleanup();
      if (typeof reader.result === 'string') resolve(reader.result);
      else reject(new Error('Preview image encoding returned no data.'));
    };
    reader.onerror = () => {
      cleanup();
      reject(new Error('Could not encode the captured preview image.'));
    };
    reader.onabort = () => {
      cleanup();
      reject(new DOMException('Preview image capture aborted.', 'AbortError'));
    };
    signal.addEventListener('abort', abort, { once: true });
    try {
      reader.readAsDataURL(blob);
    } catch (error) {
      cleanup();
      reject(error);
    }
  });
}

function describeProjectState(
  editing: boolean,
  draft: EditorDraft | undefined,
  proposal: Proposal | undefined,
  revision: Revision | undefined,
  revisions: Revision[],
  headId: string | null,
): { title: string; description: string } {
  if (editing && draft) {
    const draftBase = revisions.find((item) => item.id === draft.baseId);
    return {
      title: draft.request
        ? 'Save unconfirmed'
        : draft.baseId !== headId
          ? 'Stale draft'
          : 'Unsaved draft',
      description: draftBase
        ? `Based on revision ${draftBase.number}`
        : 'Based on an empty project',
    };
  }
  if (proposal) {
    const count = proposal.changes.length + (proposal.asset_changes?.length ?? 0);
    let description = `${count} changed file${count === 1 ? '' : 's'}`;
    if (proposal.status === 'pending') description += ' · not applied';
    if (proposal.status === 'applied') {
      const applied = revisions.find((item) => item.id === proposal.applied_revision_id);
      description += ` · revision ${applied?.number ?? 'not loaded'}`;
    }
    return {
      title: proposal.status === 'pending' ? 'Review changes' : `Proposal ${proposal.status}`,
      description,
    };
  }
  return {
    title: revision && revision.id !== headId ? 'History' : 'Current files',
    description: revision ? `Revision ${revision.number}` : 'Empty project',
  };
}

type ProposalReviewProps = {
  proposal: Proposal;
  base: Revision | undefined;
  missingBase: boolean;
  stale: boolean;
  busy: boolean;
  working: boolean;
  error: string;
  rejectionReason: string;
  onRejectionReasonChange: (value: string) => void;
  onSettle: (action: 'apply' | 'reject') => void;
};

function ProposalReviewBanner({
  proposal,
  base,
  missingBase,
  stale,
  busy,
  working,
  error,
  rejectionReason,
  onRejectionReasonChange,
  onSettle,
}: ProposalReviewProps) {
  const count = proposal.changes.length + (proposal.asset_changes?.length ?? 0);
  return (
    <div className="proposal-review">
      <strong>
        {proposal.status === 'pending' ? 'Review proposed changes' : `Proposal ${proposal.status}`}
      </strong>
      <p>
        Base:{' '}
        {base ? `Revision ${base.number}` : missingBase ? 'Revision not loaded' : 'Empty project'} ·{' '}
        {count} changed file{count === 1 ? '' : 's'}
      </p>
      {proposal.execution_id && (
        <p className="artifact-note">
          Source: execution output · {proposal.execution_id.slice(0, 8)}
        </p>
      )}
      {proposal.edit_id && (
        <p className="artifact-note">Source: manual edit · {proposal.edit_id.slice(0, 8)}</p>
      )}
      {proposal.asset_ids && (
        <p className="artifact-note">
          Source: workspace assets · {proposal.asset_ids.length} file
          {proposal.asset_ids.length === 1 ? '' : 's'} ({proposal.asset_ids[0]?.slice(0, 8)})
        </p>
      )}
      {proposal.rejection_reason && (
        <p className="artifact-note">Reason: {proposal.rejection_reason}</p>
      )}
      {missingBase && (
        <p className="artifact-note">
          Proposal saved. Refresh files to load its base revision before reviewing or applying it.
        </p>
      )}
      {proposal.status === 'pending' && (
        <>
          {stale && (
            <p className="run-error">
              The base revision is stale. Generate or run new changes against the latest revision.
            </p>
          )}
          <button
            disabled={busy || working || stale || missingBase}
            onClick={() => onSettle('apply')}
          >
            Apply changes
          </button>
          <button disabled={busy || working} onClick={() => onSettle('reject')}>
            Reject changes
          </button>
          <details className="rejection-feedback">
            <summary>Add feedback (optional)</summary>
            <label className="rejection-reason">
              Rejection reason (optional)
              <textarea
                value={rejectionReason}
                disabled={working}
                rows={2}
                onChange={(event) => onRejectionReasonChange(event.target.value)}
                placeholder="Tell the model what to change next time"
              />
            </label>
          </details>
        </>
      )}
      {error && (
        <p className="run-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

export function Artifacts({
  detail,
  selection,
  onSelection: setSelection,
  busy,
  assetsAvailable,
  refresh,
  onSettlement,
  draft,
  onDraft,
  onFix,
  onExplain,
  explainBackend,
}: Props) {
  const layout = useWorkspaceLayout();
  const [path, setPath] = useState('');
  const [view, setView] = useState<ViewMode>('source');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [previewError, setPreviewError] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const [rejectionReason, setRejectionReason] = useState('');
  const owner = useRef<AbortController | null>(null);
  const mutating = useRef(false);
  const alive = useRef(true);
  const knownProposals = useRef<Set<string> | null>(null);
  const download = useDownloads();
  useEffect(() => {
    if (selection.kind === 'proposal') {
      setView('diff');
      setRejectionReason('');
    }
  }, [selection]);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      owner.current?.abort();
    };
  }, []);

  const editing = selection.kind === 'draft' && !!draft;
  const revisions = detail?.revisions ?? [];
  const proposals = detail?.proposals ?? [];
  const revision =
    (selection.kind === 'revision' && revisions.find((revision) => revision.id === selection.id)) ||
    revisions.at(-1);
  const proposal =
    selection.kind === 'proposal'
      ? proposals.find((proposal) => proposal.id === selection.id)
      : undefined;
  const executionRevision = proposal
    ? proposal.status === 'applied'
      ? revisions.find((item) => item.id === proposal.applied_revision_id)
      : undefined
    : revision;
  const base = revisions.find((revision) => revision.id === proposal?.base_revision_id);
  const missingBase = !!proposal?.base_revision_id && !base;
  const files = proposal
    ? missingBase
      ? []
      : applyChanges(base?.files ?? [], proposal.changes)
    : (revision?.files ?? []);
  const bindings = proposal
    ? missingBase
      ? []
      : applyAssetChanges(base?.asset_bindings ?? [], proposal.asset_changes ?? [])
    : (revision?.asset_bindings ?? []);
  const images = detail?.image_assets ?? [];
  const headId = revisions.at(-1)?.id ?? null;
  const stale = !!(proposal && !missingBase && proposal.base_revision_id !== headId);
  const beforeFiles = proposal
    ? (base?.files ?? [])
    : (revisions.find((item) => item.number === (revision?.number ?? 0) - 1)?.files ?? []);
  const beforeBindings = proposal
    ? (base?.asset_bindings ?? [])
    : (revisions.find((item) => item.number === (revision?.number ?? 0) - 1)?.asset_bindings ?? []);
  const paths = missingBase
    ? []
    : [
        ...new Set(
          [
            ...files,
            ...bindings,
            ...(proposal || view === 'diff' ? [...beforeFiles, ...beforeBindings] : []),
          ].map((file) => file.path),
        ),
      ];
  const selectedPath = paths.includes(path)
    ? path
    : (proposal?.changes[0]?.path ?? proposal?.asset_changes?.[0]?.path ?? paths[0] ?? '');
  const source = files.find((file) => file.path === selectedPath);
  const binding = bindings.find((item) => item.path === selectedPath);
  const boundImage = images.find((image) => image.id === binding?.asset_id);
  const beforeBinding = beforeBindings.find((item) => item.path === selectedPath);
  const previewEntry =
    files.find((file) => file.path === selectedPath && file.path.endsWith('.html')) ??
    files.find((file) => file.path === 'index.html') ??
    files.find((file) => file.path.endsWith('.html'));
  const before = beforeFiles.find((file) => file.path === selectedPath);
  const downloadFiles = editing && draft ? draft.files : files;
  const downloadBindings = editing && draft ? (draft.assetBindings ?? []) : bindings;
  const downloadPath = editing && draft ? draft.path : selectedPath;
  const downloadSource = downloadFiles.find((file) => file.path === downloadPath);
  const diff = view === 'diff' ? fileDiff(before?.content ?? '', source?.content ?? '') : undefined;
  const canEdit =
    !missingBase &&
    (proposal ? proposal.status === 'pending' && !stale : !revision || revision.id === headId);

  useEffect(() => {
    if (!detail) return;
    const incoming = proposals.filter((item) => !knownProposals.current?.has(item.id));
    const latest = incoming.filter((item) => item.status === 'pending').at(-1);
    if (knownProposals.current && latest && !editing) showProposal(latest.id);
    knownProposals.current = new Set(proposals.map((item) => item.id));
  }, [detail]);

  useEffect(() => {
    const input = draft?.request;
    if (!input || !detail) return;
    const saved = detail.proposals.find(
      (proposal) =>
        proposal.edit_id === input.id &&
        proposal.base_revision_id === input.base_revision.id &&
        sameChanges(proposal.changes, input.changes) &&
        sameAssetChanges(proposal.asset_changes, input.asset_changes),
    );
    if (saved) {
      onDraft(detail.workspace.id, undefined);
      showProposal(saved.id, draft.path);
    }
  }, [detail, draft]);

  function startEditing() {
    if (draft) {
      setSelection({ kind: 'draft' });
      setView('source');
      return;
    }
    if (!detail || !canEdit) return;
    onDraft(detail.workspace.id, {
      baseId: proposal ? proposal.base_revision_id : (revision?.id ?? null),
      baseFiles: proposal ? (base?.files ?? []) : (revision?.files ?? []),
      baseBindings: proposal ? (base?.asset_bindings ?? []) : (revision?.asset_bindings ?? []),
      assetBindings: bindings,
      files,
      path: selectedPath,
    });
    setSelection({ kind: 'draft' });
    setView('source');
  }

  function showProposal(id: string, preferredPath = path) {
    setSelection({ kind: 'proposal', id });
    setPath(preferredPath);
    setView('diff');
    setError('');
    setRejectionReason('');
  }

  async function settle(action: 'apply' | 'reject') {
    if (!proposal || busy || mutating.current) return;
    if (action === 'reject' && new TextEncoder().encode(rejectionReason.trim()).length > 4096) {
      setError('Rejection reason must fit within 4096 UTF-8 bytes.');
      return;
    }
    const controller = new AbortController();
    owner.current = controller;
    mutating.current = true;
    setWorking(true);
    setError('');
    try {
      if (action === 'apply') {
        const applied = await api.applyProposal(proposal.id, controller.signal);
        if (controller.signal.aborted) return;
        onSettlement({ ...proposal, status: 'applied', applied_revision_id: applied.id }, applied);
        setSelection({ kind: 'current' });
        setView('source');
      } else {
        const rejected = await api.rejectProposal(proposal.id, rejectionReason, controller.signal);
        if (!controller.signal.aborted) onSettlement(rejected);
      }
      if (!controller.signal.aborted) await refresh();
    } catch (error) {
      if (!controller.signal.aborted)
        setError(error instanceof Error ? error.message : 'Cannot settle this proposal.');
    } finally {
      if (!controller.signal.aborted) setWorking(false);
      mutating.current = false;
    }
  }

  async function runPreview(target: 'selected' | 'current' = 'selected') {
    const capturedRevision = target === 'current' ? revisions.at(-1) : revision;
    const capturedProposal = target === 'current' ? undefined : proposal;
    const capturedFiles = target === 'current' ? (capturedRevision?.files ?? []) : files;
    const capturedBindings =
      target === 'current' ? (capturedRevision?.asset_bindings ?? []) : bindings;
    const entryPath = target === 'current' ? preview?.entryPath : selectedPath;
    const entry =
      capturedFiles.find((file) => file.path === entryPath && file.path.endsWith('.html')) ??
      capturedFiles.find((file) => file.path === 'index.html') ??
      capturedFiles.find((file) => file.path.endsWith('.html'));
    if (!entry || (!capturedProposal && !capturedRevision) || !detail || mutating.current) return;
    const controller = new AbortController();
    owner.current = controller;
    mutating.current = true;
    setWorking(true);
    setPreparing(true);
    setPreviewError('');
    layout.open('preview');
    try {
      const byId = new Map<string, string>();
      const resources: Record<string, string> = {};
      for (const binding of capturedBindings) {
        const image = images.find((image) => image.id === binding.asset_id);
        if (!image) throw new Error('Refresh the workspace to load the bound image metadata.');
        let url = byId.get(image.id);
        if (!url) {
          const blob = await api.imageBlob(image, controller.signal);
          controller.signal.throwIfAborted();
          url = await previewImageUrl(blob, controller.signal);
          byId.set(image.id, url);
        }
        resources[binding.path] = url;
      }
      if (controller.signal.aborted) return;
      setPreview({
        id: crypto.randomUUID(),
        source: capturedProposal
          ? { kind: 'proposal', id: capturedProposal.id }
          : { kind: 'revision', id: capturedRevision!.id, number: capturedRevision!.number },
        entryPath: entry.path,
        document: projectPreview(capturedFiles, entry.path, resources),
      });
    } catch (error) {
      if (!controller.signal.aborted)
        setPreviewError(error instanceof Error ? error.message : 'Cannot capture project preview.');
    } finally {
      mutating.current = false;
      if (!controller.signal.aborted) {
        setWorking(false);
        setPreparing(false);
      }
    }
  }

  async function exportProject() {
    if (!detail || mutating.current) return;
    const controller = new AbortController();
    owner.current = controller;
    mutating.current = true;
    setWorking(true);
    setError('');
    try {
      const blob = await api.exportProject(
        detail.workspace.id,
        downloadFiles,
        downloadBindings,
        controller.signal,
      );
      if (!controller.signal.aborted) download(blob, 'deep-project.zip');
    } catch (error) {
      if (!controller.signal.aborted)
        setError(error instanceof Error ? error.message : 'Cannot export project.');
    } finally {
      mutating.current = false;
      if (!controller.signal.aborted) setWorking(false);
    }
  }

  function exportManifest() {
    download(
      new Blob(
        [
          JSON.stringify(
            {
              files: downloadFiles,
              ...(downloadBindings.length ? { asset_bindings: downloadBindings } : {}),
            },
            null,
            2,
          ),
        ],
        { type: 'application/json' },
      ),
      editing ? 'deep-project-draft.json' : 'deep-project.json',
    );
  }

  function versionOption(next: ProjectSelection, label: string, description: string) {
    const id = 'id' in next ? next.id : next.kind === 'draft' ? 'draft' : (headId ?? 'current');
    const selected =
      selection.kind === next.kind &&
      (!('id' in next) || ('id' in selection && next.id === selection.id));
    return (
      <button
        type="button"
        key={id}
        value={id}
        data-kind={next.kind === 'current' && headId ? 'revision' : next.kind}
        className="version-option"
        aria-label={label}
        aria-current={selected ? 'true' : undefined}
        onClick={() => {
          if (editing && draft) setPath(draft.path);
          setSelection(next);
          setView(next.kind === 'proposal' ? 'diff' : 'source');
          setError('');
          layout.open('project');
        }}
      >
        <strong>{label}</strong>
        <small>{description}</small>
      </button>
    );
  }
  const draftChangeCount = draft
    ? projectChanges(draft.baseFiles, draft.files).length +
      projectAssetChanges(draft.baseBindings ?? [], draft.assetBindings ?? []).length
    : 0;
  const state = describeProjectState(editing, draft, proposal, revision, revisions, headId);
  const versionTitle = proposal
    ? `Proposal ${proposals.indexOf(proposal) + 1} · ${proposal.status}`
    : state.title;
  const previewProposal =
    preview?.source.kind === 'proposal'
      ? proposals.find((item) => item.id === preview.source.id)
      : undefined;
  const previewRevisionId =
    preview?.source.kind === 'revision' ? preview.source.id : previewProposal?.applied_revision_id;
  const previewOutdated = !!preview && previewRevisionId !== headId;
  const previewLabel = !preview
    ? ''
    : preview.source.kind === 'revision'
      ? `Previewing revision ${preview.source.number}`
      : `Previewing proposal ${previewProposal ? proposals.indexOf(previewProposal) + 1 : preview.source.id.slice(0, 8)}`;
  const currentPreviewAvailable = !!revisions
    .at(-1)
    ?.files.some((file) => file.path.endsWith('.html'));
  const previewContent = (
    <div className="preview-wrap">
      {preparing && <p role="status">Capturing approved project image resources…</p>}
      {previewError && (
        <p role="alert" className="run-error">
          {previewError}
        </p>
      )}
      {preview ? (
        <>
          <div className={`preview-label${previewOutdated ? ' is-outdated' : ''}`}>
            <span role="status" aria-label="Preview capture">
              {previewLabel}
              {previewOutdated &&
                ` · Current is ${revisions.at(-1) ? `revision ${revisions.at(-1)!.number}` : 'an empty project'}`}
              {previewProposal?.applied_revision_id &&
                ` · Applied as revision ${revisions.find((item) => item.id === previewProposal.applied_revision_id)?.number ?? 'not loaded'}`}
            </span>
            {previewOutdated && (
              <button
                type="button"
                disabled={working || !currentPreviewAvailable}
                title="Capture the current accepted revision"
                onClick={() => void runPreview('current')}
              >
                Update preview
              </button>
            )}
            <button type="button" onClick={() => setPreview(null)}>
              Stop preview
            </button>
          </div>
          <iframe
            key={preview.id}
            title="Generated app preview"
            sandbox="allow-scripts"
            referrerPolicy="no-referrer"
            srcDoc={preview.document}
          />
        </>
      ) : (
        !preparing && (
          <p className="workspace-empty">
            Open an HTML project and choose Run preview in the Editor. The preview opens here after
            that action.
          </p>
        )
      )}
    </div>
  );

  return (
    <section className="artifacts" aria-label="App revisions">
      <div className="panel-heading">
        <span>Project files</span>
        <span className="muted">
          {editing && draft
            ? draft.files.length + (draft.assetBindings?.length ?? 0)
            : files.length + bindings.length}{' '}
          FILES
        </span>
      </div>
      <WorkspacePanel id="files">
        {detail ? (
          <AssetLibrary
            workspace={detail.workspace.id}
            images={images}
            assets={detail.assets}
            head={revisions.at(-1)}
            busy={busy || working}
            enabled={assetsAvailable}
            refresh={refresh}
            onProposal={(proposal) => {
              onSettlement(proposal);
              showProposal(proposal.id);
              layout.open('project');
            }}
          />
        ) : (
          <p className="workspace-empty">
            Start a conversation to create a workspace for your files and saved outputs.
          </p>
        )}
      </WorkspacePanel>
      <WorkspacePanel id="revisions">
        <div className="version-browser">
          <section aria-label="Current version">
            <h3>Current</h3>
            {versionOption(
              { kind: 'current' },
              revisions.at(-1) ? `Revision ${revisions.at(-1)!.number} · current` : 'Empty project',
              'Accepted files',
            )}
          </section>
          <section aria-label="Working versions">
            <h3>Working</h3>
            {draft &&
              versionOption(
                { kind: 'draft' },
                'Unsaved draft',
                `${draftChangeCount} change${draftChangeCount === 1 ? '' : 's'}${draft.request ? ' · save unconfirmed' : draft.baseId !== headId ? ' · stale base' : ''}`,
              )}
            {proposals.map(
              (item, index) =>
                item.status === 'pending' &&
                versionOption(
                  { kind: 'proposal', id: item.id },
                  `Proposal ${index + 1} · pending`,
                  `${item.changes.length + (item.asset_changes?.length ?? 0)} files · ${item.base_revision_id !== headId ? 'stale base' : 'ready for review'}`,
                ),
            )}
            {!draft && !proposals.some((item) => item.status === 'pending') && (
              <p>No pending changes.</p>
            )}
          </section>
          <section aria-label="Version history">
            <h3>History</h3>
            {revisions
              .slice(0, -1)
              .reverse()
              .map((item) =>
                versionOption(
                  { kind: 'revision', id: item.id },
                  `Revision ${item.number}`,
                  'Accepted snapshot',
                ),
              )}
            {proposals.map(
              (item, index) =>
                item.status !== 'pending' &&
                versionOption(
                  { kind: 'proposal', id: item.id },
                  `Proposal ${index + 1} · ${item.status}`,
                  `${item.changes.length + (item.asset_changes?.length ?? 0)} changed files`,
                ),
            )}
          </section>
        </div>
      </WorkspacePanel>
      <div className="editor-header">
        <div className="project-state" role="status">
          <button
            type="button"
            className="project-version"
            title="Open Revisions"
            value={editing ? 'draft' : (proposal?.id ?? revision?.id ?? 'current')}
            aria-label="Choose project version"
            onClick={() => layout.open('revisions')}
          >
            <strong>{versionTitle}</strong>
            <span>{state.description} ⌄</span>
          </button>
        </div>
        {detail && (
          <button
            type="button"
            className="editor-refresh"
            aria-label="Refresh files"
            title="Refresh files"
            disabled={working}
            onClick={() =>
              void refresh()
                .then(() => {
                  if (alive.current) setError('');
                })
                .catch((error) => {
                  if (alive.current)
                    setError(error instanceof Error ? error.message : 'Cannot refresh.');
                })
            }
          >
            ↻
          </button>
        )}
        {(revision || proposal || draft) && (
          <details
            className="project-export"
            onBlur={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget))
                event.currentTarget.open = false;
            }}
            onClick={(event) => {
              if ((event.target as HTMLElement).closest('button')) event.currentTarget.open = false;
            }}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.currentTarget.open = false;
                event.currentTarget.querySelector('summary')?.focus();
              }
            }}
          >
            <summary>Export</summary>
            <div className="export-options">
              {downloadSource && (
                <button
                  type="button"
                  onClick={() =>
                    download(
                      new Blob([downloadSource.content], {
                        type: downloadPath.endsWith('.html') ? 'text/html' : 'text/plain',
                      }),
                      downloadPath.split('/').at(-1)!,
                    )
                  }
                >
                  Download
                </button>
              )}
              <button type="button" onClick={exportManifest}>
                {downloadBindings.length ? 'Download project manifest' : 'Download project'}
              </button>
              <button
                type="button"
                disabled={working || !assetsAvailable}
                onClick={() => void exportProject()}
              >
                Download app bundle (.zip)
              </button>
              {downloadBindings.length > 0 && (
                <p>JSON contains source and image IDs. The app bundle includes images.</p>
              )}
            </div>
          </details>
        )}
      </div>
      {error && (!proposal || editing) && (
        <p className="run-error" role="alert">
          {error}
        </p>
      )}
      {!editing && (
        <div className="artifact-tabs editor-viewbar">
          <button className={view === 'source' ? 'selected' : ''} onClick={() => setView('source')}>
            Source
          </button>
          <button className={view === 'diff' ? 'selected' : ''} onClick={() => setView('diff')}>
            Diff
          </button>
          <div className="editor-primary-actions">
            {detail && (draft || canEdit) && (
              <button disabled={working || (!draft && busy)} onClick={startEditing}>
                {draft ? 'Resume draft' : proposal ? 'Edit copy' : 'Edit'}
              </button>
            )}
            {(proposal || revision?.id !== headId) && revisions.length > 0 && (
              <button
                onClick={() => {
                  setSelection({ kind: 'current' });
                  setView('source');
                }}
              >
                View current
              </button>
            )}
            <button disabled={!previewEntry || working} onClick={() => void runPreview()}>
              Run preview ↗
            </button>
          </div>
        </div>
      )}
      <WorkspacePanel id="activity">
        {detail && (revision || proposal || draft) ? (
          <Executions
            workspace={detail.workspace.id}
            selectedPath={editing ? (draft?.path ?? '') : selectedPath}
            observedExecutionIds={detail.runs.flatMap((run) =>
              run.tool_execution ? [run.tool_execution.id] : [],
            )}
            observeToolRun={detail.runs.some((run) => !!run.execution_revision_id && pending(run))}
            onSelectScript={(path) => {
              setPath(path);
              setView('source');
              layout.open('project');
            }}
            revision={editing ? undefined : executionRevision}
            draftActive={editing}
            revisions={revisions}
            onFix={(job) => {
              onFix(job);
              layout.open('chat');
            }}
            onExplain={
              busy || !onExplain
                ? undefined
                : async (result) => {
                    await onExplain(result);
                    if (alive.current) layout.open('chat');
                  }
            }
            explainBackend={explainBackend}
            onProposal={async (proposal) => {
              if (!alive.current) return;
              onSettlement(proposal);
              showProposal(proposal.id);
              layout.open('project');
              await refresh();
            }}
          />
        ) : (
          <p className="workspace-empty">
            Saved-script execution and its results appear here. Live model and image steps remain in
            Chat.
          </p>
        )}
      </WorkspacePanel>
      {editing && draft && detail ? (
        <ProjectEditor
          workspace={detail.workspace.id}
          draft={draft}
          headId={headId}
          busy={busy}
          view={view}
          onView={setView}
          onExport={exportManifest}
          onDraft={(next) => {
            onDraft(detail.workspace.id, next);
            if (!next) {
              setPath(draft.path);
              setSelection({ kind: 'current' });
              setView('source');
            }
          }}
          onSaved={(proposal, selectedPath) => {
            onSettlement(proposal);
            onDraft(detail.workspace.id, undefined);
            showProposal(proposal.id, selectedPath);
            void refresh().catch((error) => {
              if (alive.current)
                setError(error instanceof Error ? error.message : 'Cannot refresh files.');
            });
          }}
        />
      ) : revision || proposal ? (
        <>
          <div className="file-workspace">
            <nav className="file-tree" aria-label="File list">
              {paths.map((name) => (
                <button
                  key={name}
                  className={
                    [...(proposal?.changes ?? []), ...(proposal?.asset_changes ?? [])].some(
                      (change) => change.path === name,
                    )
                      ? 'file-changed'
                      : ''
                  }
                  aria-current={name === selectedPath ? 'true' : undefined}
                  onClick={() => setPath(name)}
                >
                  {name}
                </button>
              ))}
            </nav>
            <div className="code-pane">
              {view === 'source' &&
                (boundImage ? (
                  <div className="bound-image" aria-label="Project image asset">
                    <p>
                      {selectedPath} → {boundImage.id}
                    </p>
                    <img
                      src={`/api/image-assets/${boundImage.id}/content`}
                      alt={`Project asset ${selectedPath}`}
                    />
                    <p>
                      Immutable image · {boundImage.width} × {boundImage.height}. Pixels are not
                      source text.
                    </p>
                  </div>
                ) : source ? (
                  <CodeEditor
                    identity={`${detail?.workspace.id}:${proposal?.id ?? revision?.id}:source`}
                    path={selectedPath}
                    value={source.content}
                  />
                ) : (
                  <p className="artifact-note">
                    {selectedPath
                      ? 'This file is deleted by the proposal.'
                      : 'This revision has no files.'}
                  </p>
                ))}
              {view === 'diff' && !missingBase && (
                <div className="file-diff" aria-label="File diff">
                  <p className="artifact-note">
                    {selectedPath} ·{' '}
                    {proposal ? 'Proposed changes against base' : 'Changes from previous revision'}
                  </p>
                  {binding || beforeBinding ? (
                    <div aria-label="Asset binding diff">
                      <p>Before: {beforeBinding?.asset_id ?? 'No binding'}</p>
                      <p>
                        After:{' '}
                        {binding?.asset_id ??
                          'Binding removed; the asset remains in the workspace library.'}
                      </p>
                      {boundImage && (
                        <img
                          className="binding-preview"
                          src={`/api/image-assets/${boundImage.id}/content`}
                          alt="Proposed project image"
                        />
                      )}
                    </div>
                  ) : diff ? (
                    <CodeEditor
                      identity={`${detail?.workspace.id}:${proposal?.id ?? revision?.id}:diff`}
                      path={selectedPath}
                      original={before?.content ?? ''}
                      value={source?.content ?? ''}
                    />
                  ) : (
                    <p className="artifact-note">
                      Diff exceeds the display budget. View Source or download the project for the
                      full content.
                    </p>
                  )}
                </div>
              )}
            </div>
          </div>
        </>
      ) : (
        <div className="artifact-empty">
          <div className="empty-window">
            <span />
            <span />
            <span />
            <div>your next small thing</div>
          </div>
          <h3>Ideas deserve a history.</h3>
          <p>
            Ask for changes to your project files, review the diff, then apply and preview or run.
          </p>
          <span className="empty-pill">No automatic code execution</span>
        </div>
      )}
      {proposal && !editing && (
        <ProposalReviewBanner
          proposal={proposal}
          base={base}
          missingBase={missingBase}
          stale={stale}
          busy={busy}
          working={working}
          error={error}
          rejectionReason={rejectionReason}
          onRejectionReasonChange={setRejectionReason}
          onSettle={(action) => void settle(action)}
        />
      )}
      <WorkspacePane id="preview">{previewContent}</WorkspacePane>
    </section>
  );
}
