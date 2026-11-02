import { Fragment, useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { api, executing, pending, imagePending, modelCleanupPending } from './api';
import type {
  Execution,
  Proposal,
  Revision,
  Run,
  ServiceStatus,
  Workspace,
  WorkspaceDetail,
} from './api';
import { ComputeResultCard } from './ComputeResultCard';
import { ImageJobDetails, ImageResultCard } from './ImageResultCard';
import { ImageAttachmentPicker, ImageAttachmentView } from './ImageAttachments';
import { ExecutionResultView } from './ExecutionResultView';
import { RunActivity } from './RunActivity';
import { Artifacts } from './Artifacts';
import type { ProjectSelection } from './Artifacts';
import {
  WorkspaceLayout,
  WorkspacePane,
  WorkspacePanel,
  WorkspaceLaunch,
  useWorkspaceLayout,
} from './WorkspaceLayout';
import { ModelInventory } from './ModelInventory';
import { ImageInventory } from './ImageInventory';
import { projectChanges, projectAssetChanges } from './project';
import type { EditorDraft } from './project';

type Settlement = { proposal: Proposal; revision?: Revision };

const errorText = (error: unknown) =>
  error instanceof Error ? error.message : 'Something went wrong.';
const aborted = (error: unknown) => error instanceof DOMException && error.name === 'AbortError';

function runSummary(run: Run) {
  const { stats } = run;
  return [
    run.model_route &&
      `${run.model_route.automatic ? `Auto (${run.model_route.task})` : 'Selected profile'} · ${run.model_route.label} · ${run.model_route.reason}`,
    stats.reported_model && `Reported model: ${stats.reported_model}`,
    run.duration_ms !== null && `${(run.duration_ms / 1000).toFixed(2)} s`,
    run.queue_ms !== null && `queue ${(run.queue_ms / 1000).toFixed(2)} s`,
    stats.first_token_ms !== null && `first text ${(stats.first_token_ms / 1000).toFixed(2)} s`,
    stats.input_tokens === null && stats.output_tokens === null
      ? 'Token usage unavailable'
      : `${stats.input_tokens ?? 'unknown'} in / ${stats.output_tokens ?? 'unknown'} out tokens`,
    stats.finish_reason && `ended: ${stats.finish_reason}`,
  ]
    .filter((value) => value !== false && value !== null)
    .join(' · ');
}

function mergeSettlements(
  loaded: WorkspaceDetail | null,
  workspaceId: string | null,
  settlements: ReadonlyMap<string, Settlement>,
): WorkspaceDetail | null {
  if (!loaded || loaded.workspace.id !== workspaceId) return null;
  const revisionMap = new Map(loaded.revisions.map((revision) => [revision.id, revision]));
  const proposalMap = new Map(loaded.proposals.map((proposal) => [proposal.id, proposal]));
  for (const { proposal, revision } of settlements.values()) {
    if (proposal.workspace_id !== loaded.workspace.id) continue;
    if (revision && !revisionMap.has(revision.id)) revisionMap.set(revision.id, revision);
    const current = proposalMap.get(proposal.id);
    if (!current || current.status === 'pending' || proposal.status !== 'pending')
      proposalMap.set(proposal.id, proposal);
  }
  return {
    ...loaded,
    revisions: [...revisionMap.values()].sort((a, b) => a.number - b.number),
    proposals: [...proposalMap.values()],
  };
}

function ProposalOutcome({ proposal, revisions }: { proposal: Proposal; revisions: Revision[] }) {
  const applied = revisions.find((revision) => revision.id === proposal.applied_revision_id);
  return (
    <div className={`proposal-outcome ${proposal.status}`} aria-label="Proposal outcome">
      <strong>
        {proposal.status === 'applied'
          ? `Applied · Revision ${applied?.number ?? 'not loaded'}`
          : proposal.status === 'rejected'
            ? 'Rejected · accepted files unchanged'
            : 'Pending review · accepted files unchanged'}
      </strong>
      {proposal.rejection_reason && <p>Reason: {proposal.rejection_reason}</p>}
    </div>
  );
}

type SidebarProps = {
  workspaces: Workspace[];
  workspaceId: string | null;
  drafts: ReadonlyMap<string, EditorDraft>;
  submitting: boolean;
  deleting: Workspace | null;
  deleteError: string;
  status: ServiceStatus | null;
  selectedBackend?: { id: string; model: string };
  onSelectWorkspace: (id: string) => void;
  onPromptDelete: (workspace: Workspace) => void;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
};

function WorkspaceSidebar({
  workspaces,
  workspaceId,
  drafts,
  submitting,
  deleting,
  deleteError,
  status,
  selectedBackend,
  onSelectWorkspace,
  onPromptDelete,
  onCancelDelete,
  onConfirmDelete,
}: SidebarProps) {
  const layout = useWorkspaceLayout();
  return (
    <aside className="sidebar">
      <div className="section-label">
        YOUR WORKSPACES <span>{workspaces.length.toString().padStart(2, '0')}</span>
      </div>
      <nav aria-label="Workspaces">
        {workspaces.map((item) => (
          <div className="workspace-row" key={item.id}>
            <button
              className={item.id === workspaceId ? 'workspace active' : 'workspace'}
              disabled={submitting}
              onClick={() => {
                onSelectWorkspace(item.id);
                layout.open('chat');
              }}
            >
              <span>▧</span>
              {item.title}
              {drafts.has(item.id) && <small className="draft-badge">Draft</small>}
            </button>
            <button
              className="workspace-delete"
              aria-label="Delete workspace"
              title={`Delete ${item.title}`}
              disabled={submitting}
              onClick={() => onPromptDelete(item)}
            >
              ×
            </button>
          </div>
        ))}
      </nav>
      {deleting && (
        <section className="delete-confirmation" aria-label="Delete workspace confirmation">
          <strong>Delete {deleting.title}?</strong>
          <p>
            This permanently removes its chat, files, proposals and execution history
            {drafts.has(deleting.id) ? ', including the unsaved draft' : ''}.
          </p>
          {deleteError && (
            <p role="alert" className="run-error">
              {deleteError}
            </p>
          )}
          <button disabled={submitting} onClick={onCancelDelete}>
            Cancel
          </button>
          <button disabled={submitting} onClick={onConfirmDelete}>
            {submitting ? 'Deleting…' : 'Delete workspace'}
          </button>
        </section>
      )}
      <div className="sidebar-note">
        <span className="status-dot" />
        {selectedBackend?.id === 'demo'
          ? 'Offline demo'
          : status
            ? 'Model backend configured'
            : 'Connecting…'}
        <small>
          {selectedBackend?.id === 'demo'
            ? 'No model or GPU required'
            : (selectedBackend?.model ?? 'Waiting for the Rust service')}
        </small>
      </div>
    </aside>
  );
}

type ConversationMessagesProps = {
  detail: WorkspaceDetail | null;
  busy: boolean;
  onSelectSuggestion: (mode: string, prompt: string) => void;
  onSettlement: (proposal: Proposal, revision?: Revision) => void;
  onReview: (proposal: Proposal) => void;
  refresh: () => Promise<unknown>;
  onImageReconcile: () => void;
  onDeleteTurn: (runId: string) => void;
  onBranchFromTurn: (runId: string) => void;
  guestMode: boolean;
};

function ConversationMessages({
  detail,
  busy,
  onSelectSuggestion,
  onSettlement,
  onReview,
  refresh,
  onImageReconcile,
  onDeleteTurn,
  onBranchFromTurn,
  guestMode,
}: ConversationMessagesProps) {
  const layout = useWorkspaceLayout();
  const [inspectedRun, setInspectedRun] = useState<string | null>(null);
  const [inspectedImage, setInspectedImage] = useState<{ runId: string; expanded: boolean } | null>(
    null,
  );
  const runDetail = detail?.runs.find((run) => run.id === inspectedRun);
  const imageDetail = detail?.runs.find((run) => run.id === inspectedImage?.runId)?.image_job;
  function review(proposal: Proposal) {
    onReview(proposal);
    layout.open('project');
  }

  function renderRunTools(run: Run) {
    if (!detail) return null;
    return (
      <>
        <RunActivity
          run={run}
          onInspect={() => {
            setInspectedRun(run.id);
            layout.open('runDetails');
          }}
        >
          <p className="run-stats">{runSummary(run)}</p>
        </RunActivity>
        {run.node_scope === 'compute' && run.tool_executions.length > 0 && (
          <ComputeResultCard
            workspaceId={detail.workspace.id}
            run={run}
            baseRevisionId={detail.revisions.at(-1)?.id ?? null}
            busy={busy}
            onSettlement={(proposal) => {
              onSettlement(proposal);
              review(proposal);
            }}
            refresh={refresh}
          />
        )}
        {run.image_job && (
          <ImageResultCard
            key={run.image_job.id}
            job={run.image_job}
            onReconcile={onImageReconcile}
            onInspect={(expanded) => {
              setInspectedImage({ runId: run.id, expanded });
              layout.open('image');
            }}
            operatorResolution={!guestMode}
          />
        )}
      </>
    );
  }

  return (
    <div className="messages" aria-live="polite">
      <WorkspacePanel id="runDetails">
        {runDetail ? (
          <section className="resource-inspector" aria-label="Saved run details">
            <h2>
              {runDetail.provider} · {runDetail.model}
            </h2>
            <p>
              {runDetail.status} · Run {runDetail.id}
            </p>
            <p>{runSummary(runDetail)}</p>
            {runDetail.error && <p className="run-error">{runDetail.error}</p>}
            <RunActivity run={runDetail} />
            <details open>
              <summary>Messages</summary>
              {detail?.messages
                .filter((message) => message.run_id === runDetail.id)
                .map((message, index) => (
                  <div key={index}>
                    <strong>{message.role}</strong>
                    <pre>{message.content}</pre>
                  </div>
                ))}
            </details>
            <details>
              <summary>All saved run data</summary>
              <pre>{JSON.stringify(runDetail, null, 2)}</pre>
            </details>
          </section>
        ) : (
          <p className="workspace-empty">Choose a run in Chat to inspect its saved details.</p>
        )}
      </WorkspacePanel>
      <WorkspacePanel id="image" expanded={inspectedImage?.expanded}>
        {imageDetail ? (
          <section className="resource-inspector image-inspector">
            {imageDetail.output_image && (
              <img
                className="inspected-image"
                hidden={!inspectedImage?.expanded}
                src={`/api/image-assets/${imageDetail.output_image.id}/content`}
                alt={`Generated image using ${imageDetail.profile.label}`}
              />
            )}
            {!inspectedImage?.expanded && (
              <>
                <ImageJobDetails job={imageDetail} />
                <WorkspaceLaunch id="files">Files & assets · use in project</WorkspaceLaunch>
              </>
            )}
          </section>
        ) : (
          <p className="workspace-empty">Choose a generated image in Chat to inspect it.</p>
        )}
      </WorkspacePanel>
      {!detail?.messages.length && !detail?.proposals.length && (
        <div className="welcome">
          <div className="welcome-symbol">✳</div>
          <span className="eyebrow">START WITH A SMALL IDEA</span>
          <h2>
            Make something
            <br />
            worth keeping.
          </h2>
          <p>
            Talk it through, build a small app, and keep every revision. Your next idea won’t erase
            the last one.
          </p>
          <button
            className="suggestion"
            onClick={() =>
              onSelectSuggestion(
                'files',
                'Build a small counter app with a calm green theme. Use index.html, style.css and app.js directly.',
              )
            }
          >
            Try a tiny counter app <span>↗</span>
          </button>
          <button
            className="suggestion"
            onClick={() => onSelectSuggestion('chat', 'Help me plan a useful local AI workspace.')}
          >
            Think through an idea <span>↗</span>
          </button>
        </div>
      )}
      {detail?.messages.map((message, index) => {
        const run = detail.runs.find((r) => r.id === message.run_id);
        const proposal =
          message.role === 'assistant'
            ? detail.proposals.find((proposal) => proposal.run_id === message.run_id)
            : undefined;
        const isTerminalForRun =
          message.role === 'assistant' || (message.role === 'user' && run?.status !== 'succeeded');
        const turnSettled =
          isTerminalForRun &&
          !!run &&
          !pending(run) &&
          !imagePending(run.image_job) &&
          !modelCleanupPending(run) &&
          run.local_model?.phase !== 'blocked';
        return (
          <Fragment key={index}>
            <article className={`message ${message.role}`}>
              <div className="message-label">
                {message.role === 'user' ? 'YOU' : 'DEEP'}
                <span>
                  {message.role === 'assistant' && run
                    ? `${run.provider} · ${run.model}`
                    : run?.status !== 'succeeded'
                      ? run?.status
                      : ''}
                </span>
              </div>
              {message.role === 'user' && run && (
                <ImageAttachmentView images={run.image_attachments} />
              )}
              {proposal && run?.workspace_tools ? (
                <>
                  <pre>{message.content}</pre>
                  <ProposalOutcome proposal={proposal} revisions={detail.revisions} />
                </>
              ) : proposal ? (
                <>
                  <p>
                    Proposed changes to {proposal.changes.length} file
                    {proposal.changes.length === 1 ? '' : 's'}. Review the proposal in Project
                    files.
                  </p>
                  <ProposalOutcome proposal={proposal} revisions={detail.revisions} />
                  <details>
                    <summary>Model response</summary>
                    <pre>{message.content}</pre>
                  </details>
                </>
              ) : (
                <pre>{message.content}</pre>
              )}
              {proposal && (
                <button type="button" className="review-project" onClick={() => review(proposal)}>
                  {proposal.status === 'pending' ? 'Review changes' : 'View changes'}
                </button>
              )}
              {message.role === 'assistant' && run && renderRunTools(run)}
              {message.role === 'user' && run?.execution_result && (
                <ExecutionResultView
                  result={run.execution_result}
                  revisions={detail.revisions}
                  included
                />
              )}
              {message.role === 'user' && run && run.status !== 'succeeded' && renderRunTools(run)}
              {message.role === 'user' && run?.error && <p className="run-error">{run.error}</p>}
              {turnSettled && !run.output && (
                <div className="turn-actions">
                  <button
                    type="button"
                    className="ghost-action"
                    disabled={busy}
                    onClick={() => onBranchFromTurn(message.run_id)}
                  >
                    Branch from here
                  </button>
                  <button
                    type="button"
                    className="ghost-action"
                    disabled={busy}
                    onClick={() => onDeleteTurn(message.run_id)}
                  >
                    Delete turn
                  </button>
                </div>
              )}
            </article>
            {message.role === 'user' && run?.output && run.status !== 'succeeded' && (
              <article className="message assistant draft" aria-label="Draft response">
                <div className="message-label">
                  DEEP{' '}
                  <span>
                    {pending(run) ? 'Generating…' : `Incomplete response · ${run.status}`}
                  </span>
                </div>
                <pre>{run.output}</pre>
                {turnSettled && (
                  <div className="turn-actions">
                    <button
                      type="button"
                      className="ghost-action"
                      disabled={busy}
                      onClick={() => onBranchFromTurn(message.run_id)}
                    >
                      Branch from here
                    </button>
                    <button
                      type="button"
                      className="ghost-action"
                      disabled={busy}
                      onClick={() => onDeleteTurn(message.run_id)}
                    >
                      Delete turn
                    </button>
                  </div>
                )}
              </article>
            )}
          </Fragment>
        );
      })}
    </div>
  );
}

type ContextSummaryProps = {
  workspaceTools: boolean;
  detail: WorkspaceDetail | null;
  mode: string;
  executionResult: Execution | null;
  hasUnsavedDraft: boolean;
};

function ContextSummaryDisclosure({
  workspaceTools,
  detail,
  mode,
  executionResult,
  hasUnsavedDraft,
}: ContextSummaryProps) {
  const latestRevision = detail?.revisions.at(-1);
  return (
    <details className="context-summary">
      <summary>Next request context</summary>
      <p>
        Completed conversation, including pending model proposal contents. Applied and rejected file
        proposals are summarized by file and outcome; full responses stay in chat.
      </p>
      <p>
        Accepted project: {latestRevision ? `Revision ${latestRevision.number}` : 'Empty project'}.
        {workspaceTools
          ? ' File tools can read current project files and saved workspace assets. Changes save automatically with a successful reply.'
          : mode === 'chat'
            ? ' The accepted file snapshot is not included in Conversation mode.'
            : ' Accepted file contents are included.'}
      </p>
      <p>
        Manual edits and asset imports appear in Revisions. Their paths, status and rejection
        reasons are included as workspace metadata when you send; they are not conversation messages
        or file contents. Pending changes are not accepted files.
      </p>
      {(latestRevision?.asset_bindings?.length ?? 0) > 0 && (
        <p>
          Approved image bindings: {latestRevision!.asset_bindings!.length}. Source edits include
          their paths and metadata, not image bytes. Binary images are not mounted in Node.
        </p>
      )}
      <p>
        {executionResult
          ? `Execution ${executionResult.id.slice(0, 8)} is attached to this request. `
          : 'No new execution result is attached. '}
        Results previously included in completed turns remain in conversation context; other
        execution history is not sent.
      </p>
      {hasUnsavedDraft && <p>Your unsaved editor draft is not included.</p>}
      {detail?.proposals.length ? (
        <ul>
          {detail.proposals.map((proposal, index) => (
            <li key={proposal.id}>
              Proposal {index + 1}: {proposal.status}
              {proposal.rejection_reason && ` · ${proposal.rejection_reason}`}
              <span className="context-paths">
                {[...proposal.changes, ...(proposal.asset_changes ?? [])]
                  .map((change) => change.path)
                  .join(', ')}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </details>
  );
}

export function App() {
  const [projectSelection, setProjectSelection] = useState<ProjectSelection>({ kind: 'current' });
  const [initialized, setInitialized] = useState(false);
  const [status, setStatus] = useState<ServiceStatus | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [loadedDetail, setDetail] = useState<WorkspaceDetail | null>(null);
  const [settlements, setSettlements] = useState(new Map<string, Settlement>());
  const [prompt, setPrompt] = useState('');
  const [executionResult, setExecutionResult] = useState<Execution | null>(null);
  const promptInput = useRef<HTMLTextAreaElement>(null);
  const [drafts, setDrafts] = useState(new Map<string, EditorDraft>());
  const [filesEnabled, setFilesEnabled] = useState(true);
  const [backendId, setBackendId] = useState('');
  const [modelTask, setModelTask] = useState<'conversation' | 'coding'>('conversation');
  const [webEnabled, setWebEnabled] = useState(false);
  const [computeEnabled, setComputeEnabled] = useState(true);
  const [selectedAssetIds, setSelectedAssetIds] = useState<string[]>([]);
  const [projectInput, setProjectInput] = useState<{ revisionId: string; path: string } | null>(
    null,
  );
  const [uploadingAsset, setUploadingAsset] = useState(false);
  const assetRequest = useRef<AbortController | null>(null);
  const [imageProfileId, setImageProfileId] = useState('');
  const [imageEnabled, setImageEnabled] = useState(true);
  const [referenceImageId, setReferenceImageId] = useState('');
  const [chatImageIds, setChatImageIds] = useState<string[]>([]);
  const [imagePollVersion, setImagePollVersion] = useState(0);
  const [submitting, setSubmitting] = useState(false);
  const [receipt, setReceipt] = useState<Run | null>(null);
  const [error, setError] = useState('');
  const [deleting, setDeleting] = useState<Workspace | null>(null);
  const [deleteError, setDeleteError] = useState('');
  const deletedIds = useRef(new Set<string>());
  const selectedWorkspace = useRef<string | null>(null);
  const loadSequence = useRef(0);
  const workspaceRequest = useRef<AbortController | null>(null);
  const mutationPending = useRef(false);

  const detail = mergeSettlements(loadedDetail, workspaceId, settlements);
  const projectHead = detail?.revisions.at(-1);
  const projectInputPath =
    projectInput?.revisionId === projectHead?.id ? (projectInput?.path ?? '') : '';
  const receiptId = receipt?.workspace_id === workspaceId ? receipt.id : undefined;
  const activeRun =
    receipt?.workspace_id === workspaceId && pending(receipt)
      ? receipt
      : detail?.workspace.id === workspaceId
        ? detail.runs.find((run) => run.id !== receipt?.id && pending(run))
        : undefined;
  const selectedProfile = status?.model_catalog.profiles.find(
    (profile) => `profile:${profile.id}` === backendId,
  );
  const autoAvailable =
    !!status && Object.values(status.model_catalog.preferences).some((ids) => ids.length);
  const selectedBackend =
    backendId === 'auto' && autoAvailable
      ? { id: 'auto', model: 'Auto' }
      : selectedProfile
        ? { id: selectedProfile.backend, model: selectedProfile.model }
        : status?.backends.find((backend) => backend.id === backendId);
  const modelSelection =
    backendId === 'auto'
      ? { model_selection: { mode: 'auto' as const, task: modelTask } }
      : selectedProfile
        ? { model_selection: { mode: 'profile' as const, profile_id: selectedProfile.id } }
        : { backend: selectedBackend?.id };
  const workspaceTools = filesEnabled && !!selectedBackend && selectedBackend.id !== 'demo';
  const mode = filesEnabled && !workspaceTools ? 'files' : 'chat';
  const currentRun = useRef<Run | undefined>(undefined);
  currentRun.current = activeRun;
  const imageProfile = status?.image_profiles.find((profile) => profile.id === imageProfileId);
  function clearImagePermission() {
    setImageEnabled(false);
    setReferenceImageId('');
  }
  const imageAllowed =
    imageEnabled && !!imageProfile && selectedBackend?.id !== 'demo' && !!selectedBackend;
  const canSearch =
    !!status?.search_available && !!selectedBackend && selectedBackend.id !== 'demo';
  const canImport =
    !!status?.url_import_available && !!selectedBackend && selectedBackend.id !== 'demo';
  const canUseWeb = canSearch || canImport;
  const canCompute =
    !!status?.execution_available && !!selectedBackend && selectedBackend.id !== 'demo';
  const computeAllowed = canCompute && computeEnabled;
  const needsTools = workspaceTools || (webEnabled && canUseWeb) || computeAllowed || imageAllowed;
  const canAttachImages =
    !!status?.chat_image_input_available &&
    (!!selectedProfile?.vision ||
      (backendId === 'auto' &&
        status.model_catalog.preferences[modelTask].some((id) =>
          status.model_catalog.profiles.some(
            (profile) => profile.id === id && profile.vision && (!needsTools || profile.tool_calls),
          ),
        )));
  function fixFromResult(job: Execution) {
    if (job.workspace_id !== selectedWorkspace.current || executing(job) || submitting) return;
    setExecutionResult(job);
    setFilesEnabled(true);
    if (!executionResult || !prompt.trim())
      setPrompt((current) =>
        [
          current.trim(),
          'Use the attached execution result to save the smallest file changes that fix the failure or requested behavior. ' +
            'Use the current accepted project files, preserve unrelated files, and do not claim the changes were tested. ' +
            'Explain which changes were made and which checks were actually run.',
        ]
          .filter(Boolean)
          .join('\n\n'),
      );
    promptInput.current?.focus();
    promptInput.current?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }

  async function explainResult(job: Execution) {
    if (job.workspace_id !== selectedWorkspace.current || executing(job)) return;
    if (!selectedBackend || selectedBackend.id === 'demo') {
      throw new Error('Select a real model backend to explain this result.');
    }
    if (mutationPending.current || currentRun.current) {
      throw new Error('Wait for the current request to finish, then choose Explain result.');
    }
    mutationPending.current = true;
    setSubmitting(true);
    setError('');
    try {
      const accepted = await api.submit(job.workspace_id, {
        prompt:
          'Explain this saved execution result. Summarize stdout, stderr and exit status, and explain any failure. This result belongs to the recorded script and revision. Do not run code again or propose project changes.',
        mode: 'chat',
        ...modelSelection,
        execution_result_id: job.id,
      });
      setReceipt(accepted);
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  const hasUnsavedEdits = [...drafts.values()].some(
    (draft) =>
      draft.request ||
      projectChanges(draft.baseFiles, draft.files).length ||
      projectAssetChanges(draft.baseBindings ?? [], draft.assetBindings ?? []).length,
  );
  useEffect(() => {
    if (!hasUnsavedEdits) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [hasUnsavedEdits]);

  function selectWorkspace(id: string | null, preserveRequest = false) {
    if (selectedWorkspace.current === id) return;
    setProjectSelection({ kind: id && drafts.has(id) ? 'draft' : 'current' });
    const continuingRequest = preserveRequest && selectedWorkspace.current === null;
    selectedWorkspace.current = id;
    loadSequence.current++;
    workspaceRequest.current?.abort();
    setWorkspaceId(id);
    setExecutionResult(null);
    if (!continuingRequest) {
      assetRequest.current?.abort();
      setWebEnabled(false);
      setComputeEnabled(true);
      setSelectedAssetIds([]);
      setProjectInput(null);
      setReferenceImageId('');
      setChatImageIds([]);
    }
    setSettlements(new Map());
  }

  function acceptSettlement(proposal: Proposal, revision?: Revision) {
    if (selectedWorkspace.current !== proposal.workspace_id) return;
    setSettlements((current) => new Map(current).set(proposal.id, { proposal, revision }));
  }

  async function refreshWorkspace(id: string) {
    if (selectedWorkspace.current !== id) return;
    const sequence = ++loadSequence.current;
    workspaceRequest.current?.abort();
    const controller = new AbortController();
    workspaceRequest.current = controller;
    const data = await api.workspace(id, controller.signal);
    if (selectedWorkspace.current === id && sequence === loadSequence.current) {
      setDetail(data);
      setReceipt((current) =>
        current?.workspace_id === id
          ? (data.runs.find((run) => run.id === current.id) ?? current)
          : current,
      );
    }
    return data;
  }

  useEffect(() => {
    let alive = true;
    const controller = new AbortController();
    let statusRetry: ReturnType<typeof setTimeout>;
    let workspaceRetry: ReturnType<typeof setTimeout>;
    let sessionRetry: ReturnType<typeof setTimeout>;
    const loadStatus = () => {
      api
        .status(controller.signal)
        .then((service) => {
          if (alive) {
            setStatus(service);
            setBackendId(
              (current) =>
                current ||
                (service.model_catalog.default_profile
                  ? `profile:${service.model_catalog.default_profile}`
                  : service.provider),
            );
            setImageProfileId((current) => current || service.image_profiles[0]?.id || '');
          }
        })
        .catch((e) => {
          if (alive && !aborted(e)) {
            setError(errorText(e));
            statusRetry = setTimeout(loadStatus, 2000);
          }
        });
    };
    const loadWorkspaces = () => {
      api
        .workspaces(controller.signal)
        .then((list) => {
          if (!alive) return;
          const visible = list.filter((item) => !deletedIds.current.has(item.id));
          setWorkspaces(visible);
          selectWorkspace(visible[0]?.id ?? null);
          setInitialized(true);
        })
        .catch((e) => {
          if (alive && !aborted(e)) {
            setError(errorText(e));
            workspaceRetry = setTimeout(loadWorkspaces, 2000);
          }
        });
    };
    const initialize = async () => {
      try {
        const session = await api.initialize(controller.signal);
        if (!alive) return;
        if (!session.guest_mode) setInitialized(true);
        loadStatus();
        loadWorkspaces();
      } catch (error) {
        if (alive && !aborted(error)) {
          setError(errorText(error));
          sessionRetry = setTimeout(initialize, 2000);
        }
      }
    };
    void initialize();
    return () => {
      alive = false;
      clearTimeout(statusRetry);
      clearTimeout(workspaceRetry);
      clearTimeout(sessionRetry);
      controller.abort();
    };
  }, []);

  useEffect(() => {
    setDetail(null);
  }, [workspaceId]);

  useEffect(() => {
    if (!workspaceId) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const data = await refreshWorkspace(workspaceId);
        if (!alive) return;
        if (
          data?.runs.some(
            (run) => pending(run) || imagePending(run.image_job) || modelCleanupPending(run),
          )
        )
          timer = setTimeout(poll, 500);
      } catch (e) {
        if (alive && !aborted(e)) {
          setError(errorText(e));
          timer = setTimeout(poll, 2000);
        }
      }
    };
    void poll();
    return () => {
      alive = false;
      clearTimeout(timer);
      workspaceRequest.current?.abort();
    };
  }, [receiptId, workspaceId, imagePollVersion]);

  async function createWorkspace(preserveRequest = false) {
    const created = await api.createWorkspace(`Workspace ${workspaces.length + 1}`);
    setWorkspaces((list) => [created, ...list]);
    selectWorkspace(created.id, preserveRequest);
    return created.id;
  }

  async function newWorkspace() {
    if (!initialized || mutationPending.current) return;
    mutationPending.current = true;
    setSubmitting(true);
    setError('');
    try {
      await createWorkspace();
    } catch (e) {
      setError(errorText(e));
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  async function deleteWorkspace() {
    if (!deleting || mutationPending.current) return;
    const target = deleting;
    mutationPending.current = true;
    setSubmitting(true);
    setDeleteError('');
    try {
      await api.deleteWorkspace(target.id);
      deletedIds.current.add(target.id);
      const remaining = workspaces.filter((item) => item.id !== target.id);
      setWorkspaces(remaining);
      setDrafts((current) => {
        const next = new Map(current);
        next.delete(target.id);
        return next;
      });
      if (selectedWorkspace.current === target.id) {
        selectWorkspace(remaining[0]?.id ?? null);
        setReceipt((current) => (current?.workspace_id === target.id ? null : current));
        setPrompt('');
      }
      setDeleting(null);
    } catch (error) {
      setDeleteError(errorText(error));
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  async function deleteTurn(runId: string) {
    if (!initialized || !workspaceId || activeRun || mutationPending.current) return;
    const owner = workspaceId;
    mutationPending.current = true;
    setSubmitting(true);
    setError('');
    try {
      await api.deleteTurn(owner, runId);
      if (selectedWorkspace.current === owner) {
        setDetail((current) =>
          current?.workspace.id === owner
            ? {
                ...current,
                messages: current.messages.filter((message) => message.run_id !== runId),
              }
            : current,
        );
        await refreshWorkspace(owner);
      }
    } catch (e) {
      if (selectedWorkspace.current === owner && !aborted(e)) setError(errorText(e));
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  async function branchWorkspace(upToRunId?: string) {
    if (!initialized || !workspaceId || activeRun || mutationPending.current) return;
    const owner = workspaceId;
    mutationPending.current = true;
    setSubmitting(true);
    setError('');
    try {
      const branched = await api.branchWorkspace(
        owner,
        upToRunId ? { up_to_run_id: upToRunId } : {},
      );
      setWorkspaces((list) => [branched, ...list.filter((item) => item.id !== branched.id)]);
      if (selectedWorkspace.current === owner) {
        selectWorkspace(branched.id);
      }
    } catch (e) {
      if (selectedWorkspace.current === owner && !aborted(e)) setError(errorText(e));
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  useEffect(() => () => assetRequest.current?.abort(), []);

  async function attachComputationFiles(files: Pick<File, 'name' | 'text'>[]) {
    if (!initialized || !files.length || mutationPending.current) return;
    const controller = new AbortController();
    assetRequest.current = controller;
    mutationPending.current = true;
    setUploadingAsset(true);
    setError('');
    let owner = workspaceId;
    let attached = false;
    try {
      owner ??= await createWorkspace(true);
      const id = owner;
      for (const file of files) {
        const content = await file.text();
        if (controller.signal.aborted) return;
        const created = await api.uploadAsset(id, { name: file.name, content }, controller.signal);
        if (controller.signal.aborted || selectedWorkspace.current !== id) return;
        setSelectedAssetIds((current) => [...new Set([...current, created.id])]);
        setDetail((current) =>
          current?.workspace.id === id
            ? {
                ...current,
                assets: [...current.assets.filter((asset) => asset.id !== created.id), created],
              }
            : current,
        );
      }
      attached = true;
      await refreshWorkspace(id);
    } catch (e) {
      if (!controller.signal.aborted && selectedWorkspace.current === owner && !aborted(e))
        setError(errorText(e));
    } finally {
      if (assetRequest.current === controller) assetRequest.current = null;
      mutationPending.current = false;
      setUploadingAsset(false);
    }
    return attached && !controller.signal.aborted && selectedWorkspace.current === owner;
  }

  async function attachProjectFile() {
    const selected = projectInput;
    if (selected?.revisionId !== projectHead?.id) return;
    const file = projectHead?.files.find((file) => file.path === selected?.path);
    if (!file) return;
    if (await attachComputationFiles([{ name: file.path, text: async () => file.content }]))
      setProjectInput((current) => (current === selected ? null : current));
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!initialized || !prompt.trim() || activeRun || mutationPending.current) return;
    if (!selectedBackend) {
      setError('Wait for the configured backends before sending.');
      return;
    }
    if (mode === 'files' && workspaceId && detail?.workspace.id !== workspaceId) {
      setError('Wait for this workspace’s current files before proposing changes.');
      return;
    }
    if (new TextEncoder().encode(prompt).byteLength > 32_768) {
      setError('Prompt must contain 1–32768 UTF-8 bytes. Shorten it before sending.');
      return;
    }
    if (
      imageAllowed &&
      referenceImageId &&
      !detail?.image_assets?.some(
        (image) => image.id === referenceImageId && image.workspace_id === workspaceId,
      )
    ) {
      setError('Choose a saved reference image in this workspace.');
      return;
    }
    if (chatImageIds.length && !canAttachImages) {
      setError(
        'Choose a vision model that supports the selected tools, or remove the chat image attachments.',
      );
      return;
    }
    if (
      chatImageIds.some(
        (id) =>
          !detail?.image_assets?.some(
            (image) => image.id === id && image.workspace_id === workspaceId,
          ),
      )
    ) {
      setError('Choose saved chat images in this workspace.');
      return;
    }
    mutationPending.current = true;
    setSubmitting(true);
    setError('');
    try {
      const id = workspaceId ?? (await createWorkspace(true));
      const validAssetIds =
        detail?.workspace.id === id
          ? selectedAssetIds.filter((assetId) => detail.assets.some((a) => a.id === assetId))
          : selectedAssetIds;
      const accepted = await api.submit(id, {
        prompt,
        mode,
        ...modelSelection,
        workspace_tools: workspaceTools,
        ...(mode === 'files'
          ? {
              base_revision: {
                id: detail?.workspace.id === id ? (detail.revisions.at(-1)?.id ?? null) : null,
              },
            }
          : {}),
        ...(executionResult?.workspace_id === id
          ? { execution_result_id: executionResult.id }
          : {}),
        search: webEnabled && canSearch,
        ...(webEnabled && canImport ? { allow_url_import: true } : {}),
        ...(imageAllowed ? { image_profile_id: imageProfile.id } : {}),
        ...(imageAllowed && referenceImageId
          ? { image_input: { role: 'reference' as const, asset_id: referenceImageId } }
          : {}),
        ...(chatImageIds.length ? { image_attachment_ids: chatImageIds } : {}),
        ...(computeAllowed
          ? {
              node_scope: 'compute',
              input_asset_ids: validAssetIds,
            }
          : {}),
      });
      setReceipt(accepted);
      if (selectedWorkspace.current === id) {
        setPrompt('');
        setExecutionResult(null);
        setWebEnabled(false);
        setSelectedAssetIds([]);
        setProjectInput(null);
        setReferenceImageId('');
        setChatImageIds([]);
      }
    } catch (e) {
      if (!aborted(e)) setError(errorText(e));
    } finally {
      mutationPending.current = false;
      setSubmitting(false);
    }
  }

  async function stop() {
    if (!activeRun || !workspaceId) return;
    try {
      const cancelled = await api.cancel(activeRun.id);
      setReceipt(cancelled);
    } catch (e) {
      if (!aborted(e)) setError(errorText(e));
    }
  }

  return (
    <WorkspaceLayout
      title={detail?.workspace.title ?? 'A little room to build.'}
      modelLabel={selectedProfile?.label ?? selectedBackend?.model ?? 'Loading…'}
      busy={!!activeRun || submitting || !!detail?.runs.some((run) => imagePending(run.image_job))}
      onStop={activeRun ? () => void stop() : undefined}
      onNew={() => void newWorkspace()}
      newDisabled={submitting || !initialized}
      onBranch={detail ? () => void branchWorkspace() : undefined}
      notices={
        <>
          {status?.guest_mode && (
            <p className="guest-notice">
              Your workspaces belong to this browser. Clearing site data loses access.
            </p>
          )}
          {error && (
            <div className="error" role="alert">
              {error}
              <button aria-label="Dismiss error" onClick={() => setError('')}>
                ×
              </button>
            </div>
          )}
        </>
      }
    >
      <WorkspacePanel id="models">
        <section className="model-settings" aria-label="Model settings">
          <h2>Chat model</h2>
          <select
            aria-label="Model backend"
            value={backendId}
            disabled={!status || submitting}
            onChange={(e) => {
              setBackendId(e.target.value);
            }}
          >
            {!status && <option value="">Loading backends…</option>}
            {autoAvailable && <option value="auto">Auto · task preferences</option>}
            {status?.model_catalog.profiles.map((profile) => (
              <option key={`profile:${profile.id}`} value={`profile:${profile.id}`}>
                {profile.label}
                {profile.vision && ' · Vision'} ·{' '}
                {status?.connections.find((c) => c.id === profile.backend)?.label ??
                  profile.backend}
              </option>
            ))}
            {status?.backends
              .filter((backend) => !backend.profile_required)
              .map((backend) => (
                <option key={backend.id} value={backend.id}>
                  {backend.id} · {backend.model}
                </option>
              ))}
          </select>
          {backendId === 'auto' && (
            <select
              aria-label="Task preference"
              value={modelTask}
              disabled={submitting}
              onChange={(event) => setModelTask(event.target.value as 'conversation' | 'coding')}
            >
              <option value="conversation">Conversation</option>
              <option value="coding">Coding</option>
            </select>
          )}

          {selectedBackend && (
            <p>
              {selectedBackend.id} · {selectedBackend.model}
            </p>
          )}
          {status?.model_discovery_available && status.connections.length > 0 && (
            <ModelInventory status={status} />
          )}

          {!!status?.image_profiles.length && (
            <section aria-label="Image model settings">
              <h2>Image generation</h2>
              <p>
                Choose a preset here. The Images permission in Chat controls its use for a request.
              </p>
              <label className="request-permission">
                <span>Image preset</span>
                <select
                  aria-label="Image generation profile"
                  value={imageProfileId}
                  disabled={submitting}
                  onChange={(event) => {
                    setImageProfileId(event.target.value);
                    setReferenceImageId('');
                  }}
                >
                  {(status?.image_profiles ?? []).map((profile) => (
                    <option key={profile.id} value={profile.id}>
                      {profile.label}
                    </option>
                  ))}
                </select>
              </label>
              {imageProfile && (
                <details>
                  <summary>Image preset details</summary>
                  <p>Base model: {imageProfile.model}</p>
                  <p>
                    Provider:{' '}
                    {imageProfile.provider === 'openai'
                      ? 'OpenAI · hosted image generation and editing'
                      : 'WanGP · local image runtime'}
                  </p>
                  <p>
                    LoRAs:{' '}
                    {imageProfile.loras
                      ? imageProfile.loras.join(', ') || 'None'
                      : 'Details unavailable'}
                  </p>
                  {!!imageProfile.loras?.length && (
                    <p>Strengths: {imageProfile.lora_strengths || 'Runtime defaults'}</p>
                  )}
                  {!!imageProfile.lora_catalog?.length && (
                    <>
                      <p>
                        The assistant may add these optional adapters at their configured strengths:
                      </p>
                      <ul>
                        {imageProfile.lora_catalog.map((lora) => (
                          <li key={lora.id}>
                            <strong>{lora.label}</strong> · {lora.id} · strength {lora.strength}
                            <br />
                            {lora.description}
                            {!!lora.trigger_words.length &&
                              ` · Trigger words: ${lora.trigger_words.join(', ')}`}
                          </li>
                        ))}
                      </ul>
                    </>
                  )}
                </details>
              )}
              {!status?.guest_mode && imageProfile?.provider === 'wangp' && (
                <ImageInventory
                  key={imageProfile.fingerprint}
                  id={imageProfile.id}
                  model={imageProfile.model}
                  catalog={imageProfile.lora_catalog ?? []}
                />
              )}
            </section>
          )}
        </section>
      </WorkspacePanel>
      <WorkspacePanel id="workspaces">
        <WorkspaceSidebar
          workspaces={workspaces}
          workspaceId={workspaceId}
          drafts={drafts}
          submitting={submitting || !initialized}
          deleting={deleting}
          deleteError={deleteError}
          status={status}
          selectedBackend={selectedBackend}
          onSelectWorkspace={selectWorkspace}
          onPromptDelete={(item) => {
            setDeleting(item);
            setDeleteError('');
          }}
          onCancelDelete={() => setDeleting(null)}
          onConfirmDelete={() => void deleteWorkspace()}
        />
      </WorkspacePanel>
      <WorkspacePane id="chat">
        <section className="conversation" aria-label="Conversation">
          <div className="panel-heading">
            <span>Conversation</span>
            <span className="muted">
              {selectedBackend?.id === 'demo' ? 'SCRIPTED DEMO' : 'CHAT'}
            </span>
          </div>
          <ConversationMessages
            guestMode={status?.guest_mode ?? false}
            detail={detail}
            busy={!!activeRun || submitting}
            onSelectSuggestion={(nextMode, nextPrompt) => {
              setFilesEnabled(nextMode === 'files');
              setPrompt(nextPrompt);
            }}
            onSettlement={acceptSettlement}
            onReview={(proposal) => setProjectSelection({ kind: 'proposal', id: proposal.id })}
            onImageReconcile={() => setImagePollVersion((version) => version + 1)}
            onDeleteTurn={(runId) => void deleteTurn(runId)}
            onBranchFromTurn={(runId) => void branchWorkspace(runId)}
            refresh={async () => {
              if (workspaceId) await refreshWorkspace(workspaceId);
            }}
          />
          <form className="composer" onSubmit={submit}>
            <label className="sr-only" htmlFor="prompt">
              Message
            </label>
            <textarea
              id="prompt"
              ref={promptInput}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder="What would you like to make?"
              rows={3}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey))
                  e.currentTarget.form?.requestSubmit();
              }}
            />
            {executionResult && (
              <div className="result-attachment" aria-label="Execution result attachment">
                <strong>
                  Attached: {executionResult.entry} · {executionResult.status} ·{' '}
                  {executionResult.id.slice(0, 8)}
                </strong>
                <p>
                  The selected backend receives the saved result and the last 8 KiB of
                  stdout/stderr. Captured files are not included.
                </p>
                <details>
                  <summary>Review saved result</summary>
                  <ExecutionResultView
                    result={executionResult}
                    revisions={detail?.revisions ?? []}
                  />
                </details>
                {detail?.revisions.at(-1)?.id !== executionResult.revision_id && (
                  <p>
                    This result is from an older revision; changes target the current accepted
                    files.
                  </p>
                )}
                <button
                  type="button"
                  disabled={submitting}
                  onClick={() => setExecutionResult(null)}
                >
                  Remove result
                </button>
              </div>
            )}
            <div className="composer-actions">
              {activeRun ? (
                <button className="send stop" type="button" onClick={() => void stop()}>
                  Stop request <span>■</span>
                </button>
              ) : (
                <button
                  className="send"
                  type="submit"
                  disabled={!initialized || submitting || !prompt.trim() || !selectedBackend}
                >
                  {submitting ? 'Sending…' : 'Send'} <span>↑</span>
                </button>
              )}
            </div>
            <div className="tools-bar" role="group" aria-label="Request options">
              <label className="request-permission">
                <input
                  aria-label="Change project files for this request"
                  type="checkbox"
                  checked={filesEnabled}
                  disabled={submitting}
                  onChange={(event) => {
                    setFilesEnabled(event.target.checked);
                  }}
                />
                Files
              </label>
              {(status?.search_available || status?.url_import_available) && (
                <label className="request-permission">
                  <input
                    aria-label="Web access for this request"
                    type="checkbox"
                    checked={webEnabled && canUseWeb}
                    disabled={submitting || !canUseWeb}
                    onChange={(event) => {
                      setWebEnabled(event.target.checked);
                    }}
                  />
                  Web
                </label>
              )}
              {status?.execution_available && (
                <label className="request-permission">
                  <input
                    aria-label="Analyze files and create outputs"
                    type="checkbox"
                    checked={computeAllowed}
                    disabled={submitting || !canCompute}
                    onChange={(event) => {
                      setComputeEnabled(event.target.checked);
                    }}
                  />
                  Analyze
                </label>
              )}
              {!!status?.image_profiles.length && (
                <label className="request-permission">
                  <input
                    aria-label="Image generation for this request"
                    type="checkbox"
                    checked={imageAllowed}
                    disabled={submitting || !selectedBackend || selectedBackend.id === 'demo'}
                    onChange={(event) => {
                      if (event.target.checked) {
                        setImageEnabled(true);
                        const first = status?.image_profiles[0];
                        if (!imageProfile && first) setImageProfileId(first.id);
                        setReferenceImageId('');
                      } else {
                        clearImagePermission();
                      }
                    }}
                  />
                  Images
                </label>
              )}
            </div>
            <div className="composer-tools">
              <WorkspaceLaunch id="tools">Tools & attachments</WorkspaceLaunch>
              <span>
                {[
                  filesEnabled && 'Files',
                  webEnabled && canUseWeb && 'Web',
                  computeAllowed && 'Compute',
                  imageAllowed && 'Images',
                  chatImageIds.length > 0 && `${chatImageIds.length} attached`,
                  referenceImageId && 'Image reference',
                ]
                  .filter(Boolean)
                  .join(' · ') || 'Chat only'}
              </span>
            </div>
            <WorkspacePanel id="tools">
              <div className="tools-control">
                <p className="attachment-guide">
                  Choose where each file goes. Attaching it for one tool does not attach it for the
                  others.
                </p>
                {workspaceTools && (
                  <small className="tool-disclosure">
                    File tools read your workspace and save changes automatically as a revision when
                    the reply succeeds.
                  </small>
                )}
                {!!status?.image_profiles.length && selectedBackend?.id === 'demo' && (
                  <small className="tool-disclosure">
                    Select a real model backend to enable image generation.
                  </small>
                )}
                {status?.chat_image_input_available && (
                  <ImageAttachmentPicker
                    images={detail?.image_assets ?? []}
                    selected={chatImageIds}
                    enabled={canAttachImages}
                    busy={submitting}
                    onChange={setChatImageIds}
                  />
                )}
                {imageAllowed && (
                  <div className="tool-expanded-panel">
                    <h3>Image generation reference</h3>
                    <small className="tool-disclosure">
                      The image prompt goes to{' '}
                      {imageProfile.provider === 'openai'
                        ? 'OpenAI'
                        : 'the configured WanGP runtime'}
                      . Generation returns image metadata. Attach an image to chat for visual
                      inspection. Workspace file tools can save the image into project files.
                    </small>
                    {status?.image_input_available && (
                      <label className="request-permission">
                        <span>Reference image</span>
                        <select
                          aria-label="Reference image"
                          value={referenceImageId}
                          disabled={submitting}
                          onChange={(event) => setReferenceImageId(event.target.value)}
                        >
                          <option value="">None — generate from text</option>
                          {detail?.image_assets?.map((image) => (
                            <option key={image.id} value={image.id}>
                              {image.name ?? `Image ${image.id.slice(0, 8)}`}
                            </option>
                          ))}
                        </select>
                        <small>
                          Choose an image from Assets to edit. This selection sends its pixels to{' '}
                          {imageProfile.provider === 'openai' ? 'OpenAI' : 'WanGP'}. Attach it to
                          chat separately if the chat model should inspect it too.
                        </small>
                      </label>
                    )}
                  </div>
                )}
                {webEnabled && canUseWeb && (
                  <small className="tool-disclosure">
                    {canSearch && (
                      <>
                        Search queries may include conversation or project details and go to the
                        configured search service and engine.{' '}
                      </>
                    )}
                    {canImport && (
                      <>
                        Public CSV, JSON and text files can be downloaded and saved in this
                        workspace. Enable file analysis to calculate results and create reports.
                      </>
                    )}
                  </small>
                )}
                {computeAllowed && (
                  <div className="compute-inputs" aria-label="Computation inputs">
                    <h3>Files for analysis tools</h3>
                    <small className="tool-disclosure">
                      Let AI calculate, compare or transform selected text files and save
                      downloadable results in Assets. The selected model receives filenames and
                      execution logs; logs can include file data.
                    </small>
                    <div className="compute-inputs-bar">
                      <label className="upload-asset-label">
                        <span>Upload text files from your device</span>
                        <input
                          type="file"
                          multiple
                          aria-label="Attach text files"
                          accept=".csv,.tsv,.json,.svg,.txt,.md,.html,.css,.js,.mjs,.cjs,.xml,text/*"
                          disabled={submitting || uploadingAsset}
                          onChange={(event) => {
                            const files = Array.from(event.target.files ?? []);
                            event.target.value = '';
                            void attachComputationFiles(files);
                          }}
                        />
                      </label>
                      {!prompt.trim() && (
                        <div className="execution-controls">
                          <button
                            type="button"
                            onClick={() =>
                              setPrompt(
                                'Total the numeric columns in the attached CSV and save a summary as a Markdown report.',
                              )
                            }
                          >
                            Try CSV totals
                          </button>
                          <button
                            type="button"
                            onClick={() =>
                              setPrompt(
                                'Compare the attached JSON files and save a readable report of the differences.',
                              )
                            }
                          >
                            Try JSON comparison
                          </button>
                        </div>
                      )}
                    </div>
                    <fieldset className="workspace-inputs" aria-label="Attach from workspace">
                      <legend>Attach from workspace</legend>
                      {projectHead && projectHead.files.length > 0 && (
                        <div className="workspace-project-input">
                          <label>
                            Saved Project files · Revision {projectHead.number}
                            <select
                              aria-label="Workspace project file"
                              value={projectInputPath}
                              disabled={submitting || uploadingAsset}
                              onChange={(event) =>
                                setProjectInput(
                                  event.target.value
                                    ? {
                                        revisionId: projectHead.id,
                                        path: event.target.value,
                                      }
                                    : null,
                                )
                              }
                            >
                              <option value="">Choose a saved file…</option>
                              {projectHead.files.map((file) => (
                                <option key={file.path} value={file.path}>
                                  {file.path}
                                </option>
                              ))}
                            </select>
                          </label>
                          <button
                            type="button"
                            disabled={!projectInputPath || submitting || uploadingAsset}
                            onClick={() => void attachProjectFile()}
                          >
                            Attach project file
                          </button>
                        </div>
                      )}
                      {(detail?.assets.length ?? 0) > 0 ? (
                        <div
                          className="asset-selection-list"
                          role="group"
                          aria-label="Workspace assets for analysis"
                        >
                          {detail!.assets.map((asset) => (
                            <label key={asset.id} className="asset-checkbox">
                              <input
                                type="checkbox"
                                checked={selectedAssetIds.includes(asset.id)}
                                disabled={submitting || uploadingAsset}
                                onChange={(event) =>
                                  setSelectedAssetIds((current) =>
                                    event.target.checked
                                      ? [...new Set([...current, asset.id])]
                                      : current.filter((id) => id !== asset.id),
                                  )
                                }
                              />
                              {asset.name} ({asset.bytes} B · {asset.origin})
                            </label>
                          ))}
                        </div>
                      ) : (
                        <p>
                          No text files in Assets yet. Upload a file, import a URL, or attach a
                          saved Project file.
                        </p>
                      )}
                    </fieldset>
                  </div>
                )}
              </div>
            </WorkspacePanel>
          </form>
          <p className="composer-note">
            {activeRun
              ? `${activeRun.status} · ${activeRun.provider} · ${activeRun.id.slice(0, 8)}`
              : 'Ctrl + Enter to send'}
            {selectedBackend?.id === 'demo' && ' · Demo responses are scripted'}
            {selectedBackend &&
              selectedBackend.id !== 'demo' &&
              (mode === 'chat'
                ? ' · Selected backend receives the completed conversation'
                : ' · Selected backend receives the conversation and project files')}
            {mode === 'files' &&
              ` · Base: ${detail?.revisions.at(-1) ? `Revision ${detail.revisions.at(-1)!.number}` : 'Empty project'}`}
          </p>
          <ContextSummaryDisclosure
            workspaceTools={workspaceTools}
            detail={detail}
            mode={mode}
            executionResult={executionResult}
            hasUnsavedDraft={!!(workspaceId && drafts.has(workspaceId))}
          />
        </section>
      </WorkspacePane>
      <WorkspacePane id="project">
        <Artifacts
          key={workspaceId ?? 'new'}
          detail={detail}
          selection={projectSelection}
          onSelection={setProjectSelection}
          assetsAvailable={status?.asset_library_available}
          busy={!!activeRun || submitting}
          onSettlement={acceptSettlement}
          onFix={fixFromResult}
          onExplain={selectedBackend && selectedBackend.id !== 'demo' ? explainResult : undefined}
          explainBackend={
            selectedBackend && selectedBackend.id !== 'demo'
              ? `${selectedBackend.id} · ${selectedBackend.model}`
              : undefined
          }
          draft={workspaceId ? drafts.get(workspaceId) : undefined}
          onDraft={(id, draft) => {
            if (selectedWorkspace.current !== id) return;
            setDrafts((current) => {
              const next = new Map(current);
              if (draft) next.set(id, draft);
              else next.delete(id);
              return next;
            });
          }}
          refresh={async () => {
            if (workspaceId) await refreshWorkspace(workspaceId);
          }}
        />
      </WorkspacePane>
    </WorkspaceLayout>
  );
}
