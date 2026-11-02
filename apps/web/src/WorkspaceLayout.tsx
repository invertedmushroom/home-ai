import { createContext, Fragment, useContext, useEffect, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent, PointerEvent, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import './workspace-layout.css';

const panes = { chat: 'Chat', project: 'Editor', preview: 'Preview' } as const;
const panels = {
  workspaces: 'Workspaces',
  files: 'Files & assets',
  activity: 'Runs',
  tools: 'Tools & attachments',
  revisions: 'Revisions',
  models: 'Models',
  runDetails: 'Run details',
  image: 'Image',
} as const;
type Pane = keyof typeof panes;
type Panel = keyof typeof panels;
type Target = Pane | Panel;
type Rect = { x: number; y: number; width: number; height: number };
type Bounds = { width: number; height: number };
type PanelState = { open: boolean; rect: Rect };
const panelIds = Object.keys(panels) as Panel[];

function PaneDivider({
  before,
  after,
  vertical,
  value,
  onResize,
  hidden,
}: {
  before: Pane;
  after: Pane;
  vertical: boolean;
  value: number;
  onResize: (ratio: number) => void;
  hidden: boolean;
}) {
  const gesture = useRef<{
    target: HTMLDivElement;
    pointer: number;
    start: number;
    size: number;
    total: number;
    minBefore: number;
    minAfter: number;
  } | null>(null);
  const [dragging, setDragging] = useState(false);
  function release() {
    const current = gesture.current;
    gesture.current = null;
    if (current?.target.hasPointerCapture(current.pointer))
      current.target.releasePointerCapture(current.pointer);
    setDragging(false);
  }
  useEffect(() => {
    if (!dragging) return;
    const visibility = () => {
      if (document.hidden) release();
    };
    window.addEventListener('blur', release);
    window.addEventListener('pagehide', release);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      window.removeEventListener('blur', release);
      window.removeEventListener('pagehide', release);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [dragging]);
  useEffect(() => {
    release();
  }, [hidden, vertical]);
  function measure(target: HTMLDivElement) {
    const first = target.parentElement!.querySelector<HTMLElement>(`[data-pane="${before}"]`)!;
    const second = target.parentElement!.querySelector<HTMLElement>(`[data-pane="${after}"]`)!;
    const dimension = vertical ? 'width' : 'height';
    const minimum = vertical ? 'minWidth' : 'minHeight';
    const size = first.getBoundingClientRect()[dimension];
    return {
      size,
      total: size + second.getBoundingClientRect()[dimension],
      minBefore: parseFloat(getComputedStyle(first)[minimum]) || 0,
      minAfter: parseFloat(getComputedStyle(second)[minimum]) || 0,
    };
  }
  function resize(size: number, measured: ReturnType<typeof measure>) {
    if (measured.total <= 0) return;
    onResize(
      Math.max(measured.minBefore, Math.min(size, measured.total - measured.minAfter)) /
        measured.total,
    );
  }
  return (
    <div
      role="separator"
      tabIndex={0}
      hidden={hidden}
      className={`pane-divider divider-${after}${dragging ? ' is-interacting' : ''}`}
      aria-label={`Resize ${panes[before]} and ${panes[after]}`}
      aria-orientation={vertical ? 'vertical' : 'horizontal'}
      aria-controls={`workspace-pane-${before} workspace-pane-${after}`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(value * 100)}
      title="Drag to resize. Arrow keys resize too."
      onPointerDown={(event) => {
        if (event.button !== 0 || !event.isPrimary) return;
        event.preventDefault();
        event.currentTarget.focus();
        event.currentTarget.setPointerCapture(event.pointerId);
        gesture.current = {
          target: event.currentTarget,
          pointer: event.pointerId,
          start: vertical ? event.clientX : event.clientY,
          ...measure(event.currentTarget),
        };
        setDragging(true);
      }}
      onPointerMove={(event) => {
        const current = gesture.current;
        if (!current || current.pointer !== event.pointerId) return;
        resize(current.size + (vertical ? event.clientX : event.clientY) - current.start, current);
      }}
      onPointerUp={release}
      onPointerCancel={release}
      onLostPointerCapture={release}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          release();
          return;
        }
        const delta =
          event.key === (vertical ? 'ArrowLeft' : 'ArrowUp')
            ? -16
            : event.key === (vertical ? 'ArrowRight' : 'ArrowDown')
              ? 16
              : 0;
        if (!delta) return;
        event.preventDefault();
        const measured = measure(event.currentTarget);
        resize(measured.size + delta, measured);
      }}
    />
  );
}

function constrain(rect: Rect, bounds: Bounds): Rect {
  const width = Math.min(Math.max(rect.width, 300), bounds.width);
  const height = Math.min(Math.max(rect.height, 220), bounds.height);
  return {
    width,
    height,
    x: Math.max(0, Math.min(rect.x, bounds.width - width)),
    y: Math.max(0, Math.min(rect.y, bounds.height - height)),
  };
}

type LayoutState = {
  hosts: Partial<Record<Pane, HTMLDivElement>>;
  floating: HTMLDivElement | null;
  expanded: Pane | null;
  windows: Record<Panel, PanelState>;
  order: Panel[];
  open: (id: Target) => void;
  close: (id: Panel) => void;
  raise: (id: Panel) => void;
  move: (id: Panel, rect: Rect) => void;
};
const LayoutContext = createContext<LayoutState | null>(null);
function useLayoutState() {
  const state = useContext(LayoutContext);
  if (!state) throw new Error('Workspace views need their workspace layout.');
  return state;
}
export function useWorkspaceLayout() {
  const { open } = useLayoutState();
  return { open };
}
export function WorkspaceLaunch({ id, children }: { id: Target; children: ReactNode }) {
  const { open } = useWorkspaceLayout();
  return (
    <button type="button" onClick={() => open(id)}>
      {children}
    </button>
  );
}

export function WorkspaceLayout({
  title,
  modelLabel,
  busy,
  onStop,
  onNew,
  newDisabled,
  onBranch,
  notices,
  children,
}: {
  title: string;
  modelLabel: string;
  busy: boolean;
  onStop?: () => void;
  onNew: () => void;
  newDisabled: boolean;
  onBranch?: () => void;
  notices: ReactNode;
  children: ReactNode;
}) {
  const [hosts, setHosts] = useState<LayoutState['hosts']>({});
  const [floating, setFloating] = useState<HTMLDivElement | null>(null);
  const [bounds, setBounds] = useState<Bounds>({ width: 1280, height: 800 });
  const [expanded, setExpanded] = useState<Pane | null>(null);
  const [previewHidden, setPreviewHidden] = useState(false);
  const [columns, setColumns] = useState({ chat: 0.85, project: 1.2, preview: 0.95 });
  const [rows, setRows] = useState({ project: 1, preview: 0.8 });
  const wide = bounds.width > 1100;
  const mobile = bounds.width <= 720;
  const [windows, setWindows] = useState(
    () =>
      Object.fromEntries(
        panelIds.map((id, index) => [
          id,
          {
            open: false,
            rect: {
              x: 110 + index * 32,
              y: 44 + index * 20,
              width: id === 'workspaces' ? 340 : 560,
              height: 580,
            },
          },
        ]),
      ) as Record<Panel, PanelState>,
  );
  const [order, setOrder] = useState<Panel[]>([]);
  const [reveal, setReveal] = useState<{ id: Target } | null>(null);
  const openers = useRef<Partial<Record<Panel, HTMLElement>>>({});
  const paneRefs = useRef(
    Object.fromEntries(
      (Object.keys(panes) as Pane[]).map((id) => [
        id,
        (node: HTMLDivElement | null) => {
          setHosts((current) => {
            if (current[id] === node || (!node && !current[id])) return current;
            const next = { ...current };
            if (node) next[id] = node;
            else delete next[id];
            return next;
          });
        },
      ]),
    ) as Record<Pane, (node: HTMLDivElement | null) => void>,
  );

  useEffect(() => {
    if (!floating) return;
    let current = true;
    const observer = new ResizeObserver(() => {
      if (!current || !floating.isConnected) return;
      const next = {
        width: Math.max(1, floating.clientWidth),
        height: Math.max(1, floating.clientHeight),
      };
      setBounds(next);
      setWindows(
        (previous) =>
          Object.fromEntries(
            panelIds.map((id) => [
              id,
              { ...previous[id], rect: constrain(previous[id].rect, next) },
            ]),
          ) as Record<Panel, PanelState>,
      );
    });
    observer.observe(floating);
    return () => {
      current = false;
      observer.disconnect();
    };
  }, [floating]);

  useEffect(() => {
    if (!reveal) return;
    if (reveal.id in panes)
      hosts[reveal.id as Pane]
        ?.closest<HTMLElement>('.workspace-pane')
        ?.focus({ preventScroll: false });
    else
      floating
        ?.querySelector<HTMLElement>(`[data-panel="${reveal.id}"] .workspace-tool-title`)
        ?.focus({ preventScroll: true });
  }, [reveal, hosts, floating, expanded]);

  function raise(id: Panel) {
    setOrder((current) =>
      current.at(-1) === id ? current : [...current.filter((item) => item !== id), id],
    );
  }
  function open(id: Target) {
    if (id in panes) {
      if (id === 'preview') setPreviewHidden(false);
      setExpanded((current) => (current && current !== id ? (id as Pane) : current));
      setWindows(
        (current) =>
          Object.fromEntries(
            panelIds.map((panel) => [panel, { ...current[panel], open: false }]),
          ) as Record<Panel, PanelState>,
      );
      setOrder([]);
    } else {
      if (document.activeElement instanceof HTMLElement)
        openers.current[id as Panel] = document.activeElement;
      setWindows((current) => ({ ...current, [id]: { ...current[id as Panel], open: true } }));
      raise(id as Panel);
    }
    setReveal({ id });
  }
  function close(id: Panel) {
    setWindows((current) => ({ ...current, [id]: { ...current[id], open: false } }));
    setOrder((current) => current.filter((item) => item !== id));
    const opener = openers.current[id];
    if (opener?.isConnected && opener.getClientRects().length) opener.focus();
  }
  function move(id: Panel, rect: Rect) {
    setWindows((current) => ({
      ...current,
      [id]: { ...current[id], rect: constrain(rect, bounds) },
    }));
  }
  function resizeColumns(before: 'chat' | 'project', ratio: number) {
    setColumns((current) => {
      if (before === 'chat' && previewHidden && wide) {
        const remaining = current.project + current.preview;
        const total = current.chat + remaining;
        const scale = (total * (1 - ratio)) / remaining;
        return {
          chat: total * ratio,
          project: current.project * scale,
          preview: current.preview * scale,
        };
      }
      const after = before === 'chat' ? 'project' : 'preview';
      const total = current[before] + current[after];
      return { ...current, [before]: total * ratio, [after]: total * (1 - ratio) };
    });
  }
  const editorWidth = columns.project + (previewHidden && wide ? columns.preview : 0);
  const chatHidden = !!expanded && expanded !== 'chat';
  const state = { hosts, floating, expanded, windows, order, open, close, raise, move };
  return (
    <LayoutContext.Provider value={state}>
      <div className="workspace-ui">
        <header className="workspace-toolbar">
          <span className="workspace-brand">deep</span>
          <button
            type="button"
            className="workspace-chooser"
            aria-label="Choose workspace"
            onClick={() => open('workspaces')}
          >
            {title} <span>⌄</span>
          </button>
          <button
            type="button"
            aria-label="New workspace"
            title="New workspace"
            disabled={newDisabled}
            onClick={() => {
              onNew();
              setExpanded(null);
              open('chat');
            }}
          >
            ＋
          </button>
          <button
            type="button"
            aria-label="Models"
            title={modelLabel}
            onClick={() => open('models')}
          >
            Models <span className="workspace-model-label">{modelLabel}</span>
          </button>
          <div className="workspace-tool-actions">
            <button type="button" onClick={() => open('files')}>
              Files & assets
            </button>
            <button type="button" onClick={() => open('activity')}>
              Runs
            </button>
            <button type="button" onClick={() => open('tools')}>
              Tools & attachments
            </button>
            {previewHidden && (
              <button type="button" onClick={() => open('preview')}>
                Show Preview
              </button>
            )}
            {onBranch && (
              <button type="button" disabled={busy} onClick={onBranch}>
                Branch workspace
              </button>
            )}
          </div>
          <span className="workspace-status" role="status">
            {busy ? 'Working…' : ''}
          </span>
          {onStop && chatHidden && (
            <button type="button" className="workspace-stop" onClick={onStop}>
              Stop request
            </button>
          )}
        </header>
        <div className="workspace-notices">{notices}</div>
        <main className="workspace-stage" aria-label="Workspace">
          <div
            className={`workspace-grid${expanded ? ' is-expanded' : ''}${previewHidden ? ' is-preview-hidden' : ''}`}
            style={
              {
                '--chat-size': `${columns.chat}fr`,
                '--editor-size': `${editorWidth}fr`,
                '--preview-size': `${columns.preview}fr`,
                '--upper-size': `${rows.project}fr`,
                '--lower-size': `${rows.preview}fr`,
              } as CSSProperties
            }
          >
            {(Object.keys(panes) as Pane[]).map((id) => (
              <Fragment key={id}>
                <section
                  id={`workspace-pane-${id}`}
                  className={`workspace-pane pane-${id}`}
                  data-pane={id}
                  role="region"
                  aria-label={`${panes[id]} pane`}
                  tabIndex={-1}
                  hidden={(!!expanded && expanded !== id) || (id === 'preview' && previewHidden)}
                >
                  <div
                    className="workspace-pane-body workspace-surface"
                    ref={paneRefs.current[id]}
                  />
                  {id === 'preview' && (
                    <button
                      type="button"
                      className="pane-hide"
                      aria-label="Hide Preview"
                      title="Hide Preview"
                      onClick={() => {
                        setPreviewHidden(true);
                        if (expanded === 'preview') setExpanded(null);
                        setReveal({ id: 'project' });
                      }}
                    >
                      −
                    </button>
                  )}
                  <button
                    type="button"
                    className="pane-expand"
                    aria-label={
                      expanded === id
                        ? `Restore ${panes[id]} to ${previewHidden ? 'two' : 'three'} panes`
                        : `Expand ${panes[id]}`
                    }
                    title={expanded === id ? 'Return to split view' : `Expand ${panes[id]}`}
                    onClick={() => setExpanded((current) => (current === id ? null : id))}
                  >
                    {expanded === id ? '↙' : '↗'}
                  </button>
                </section>
                {id === 'chat' && (
                  <PaneDivider
                    before="chat"
                    after="project"
                    vertical
                    value={columns.chat / (columns.chat + editorWidth)}
                    onResize={(ratio) => resizeColumns('chat', ratio)}
                    hidden={!!expanded || mobile}
                  />
                )}
                {id === 'project' && (
                  <PaneDivider
                    before="project"
                    after="preview"
                    vertical={wide}
                    value={
                      wide
                        ? columns.project / (columns.project + columns.preview)
                        : rows.project / (rows.project + rows.preview)
                    }
                    onResize={(ratio) => {
                      if (wide) resizeColumns('project', ratio);
                      else
                        setRows((current) => {
                          const total = current.project + current.preview;
                          return { project: total * ratio, preview: total * (1 - ratio) };
                        });
                    }}
                    hidden={!!expanded || mobile || previewHidden}
                  />
                )}
              </Fragment>
            ))}
          </div>
          <div className="workspace-floating-layer" ref={setFloating} />
        </main>
      </div>
      {children}
    </LayoutContext.Provider>
  );
}

export function WorkspacePane({ id, children }: { id: Pane; children: ReactNode }) {
  const { hosts } = useLayoutState();
  return hosts[id] ? createPortal(children, hosts[id], id) : null;
}

export function WorkspacePanel({
  id,
  children,
  expanded = false,
}: {
  id: Panel;
  children: ReactNode;
  expanded?: boolean;
}) {
  const layout = useLayoutState();
  const [gesture, setGesture] = useState<{
    pointer: number;
    x: number;
    y: number;
    rect: Rect;
    resize: boolean;
  } | null>(null);
  const panel = layout.windows[id];
  useEffect(() => {
    if (!panel.open || expanded) setGesture(null);
  }, [panel.open, expanded]);
  useEffect(() => {
    if (!gesture) return;
    const release = () => setGesture(null);
    const visibility = () => {
      if (document.hidden) release();
    };
    window.addEventListener('blur', release);
    window.addEventListener('pagehide', release);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      window.removeEventListener('blur', release);
      window.removeEventListener('pagehide', release);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [gesture]);
  if (!layout.floating) return null;
  function start(event: PointerEvent<HTMLButtonElement>, resize: boolean) {
    if (expanded) return;
    if (event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    layout.raise(id);
    setGesture({
      pointer: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      rect: panel.rect,
      resize,
    });
  }
  function drag(event: PointerEvent<HTMLButtonElement>) {
    if (!gesture || event.pointerId !== gesture.pointer) return;
    const dx = event.clientX - gesture.x,
      dy = event.clientY - gesture.y;
    layout.move(
      id,
      gesture.resize
        ? { ...gesture.rect, width: gesture.rect.width + dx, height: gesture.rect.height + dy }
        : { ...gesture.rect, x: gesture.rect.x + dx, y: gesture.rect.y + dy },
    );
  }
  function keyboard(event: KeyboardEvent<HTMLButtonElement>, resize = false) {
    if (event.key === 'Escape') {
      event.preventDefault();
      layout.close(id);
      return;
    }
    const directions: Record<string, [number, number]> = {
      ArrowLeft: [-16, 0],
      ArrowRight: [16, 0],
      ArrowUp: [0, -16],
      ArrowDown: [0, 16],
    };
    const direction = directions[event.key];
    if (!direction || expanded) return;
    event.preventDefault();
    layout.move(
      id,
      resize || event.shiftKey
        ? {
            ...panel.rect,
            width: panel.rect.width + direction[0],
            height: panel.rect.height + direction[1],
          }
        : { ...panel.rect, x: panel.rect.x + direction[0], y: panel.rect.y + direction[1] },
    );
  }
  const pointer = {
    onPointerMove: drag,
    onPointerUp: () => setGesture(null),
    onPointerCancel: () => setGesture(null),
    onLostPointerCapture: () => setGesture(null),
  };
  return createPortal(
    <section
      role="region"
      aria-label={`${panels[id]} window`}
      data-panel={id}
      className={`workspace-tool-window${gesture ? ' is-interacting' : ''}${expanded ? ' is-expanded' : ''}`}
      hidden={!panel.open}
      style={{
        left: expanded ? 0 : panel.rect.x,
        top: expanded ? 0 : panel.rect.y,
        width: expanded ? '100%' : panel.rect.width,
        height: expanded ? '100%' : panel.rect.height,
        zIndex: layout.order.indexOf(id) + 1,
      }}
      onPointerDown={(event) => {
        event.stopPropagation();
        layout.raise(id);
      }}
      onFocus={(event) => {
        event.stopPropagation();
        layout.raise(id);
      }}
    >
      <header className="workspace-tool-bar">
        <button
          type="button"
          className="workspace-tool-title"
          aria-label={`Move ${panels[id]} window`}
          title="Drag or use arrow keys to move. Shift + arrows resizes."
          onPointerDown={(event) => start(event, false)}
          {...pointer}
          onKeyDown={(event) => keyboard(event)}
        >
          {panels[id]}
        </button>
        <button type="button" aria-label={`Close ${panels[id]}`} onClick={() => layout.close(id)}>
          ×
        </button>
      </header>
      <div className="workspace-tool-body workspace-surface">{children}</div>
      <button
        type="button"
        className="workspace-tool-resize"
        hidden={expanded}
        aria-label={`Resize ${panels[id]} window`}
        onPointerDown={(event) => start(event, true)}
        {...pointer}
        onKeyDown={(event) => keyboard(event, true)}
      >
        ◢
      </button>
    </section>,
    layout.floating,
    id,
  );
}
