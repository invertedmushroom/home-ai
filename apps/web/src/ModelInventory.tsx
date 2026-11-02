import { useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { ModelInventory as Inventory, ServiceStatus } from './api';

export function ModelInventory({ status }: { status: ServiceStatus }) {
  const [selected, setSelected] = useState(status.connections[0]?.id ?? '');
  const [snapshots, setSnapshots] = useState(new Map<string, Inventory>());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const request = useRef<AbortController | null>(null);
  useEffect(() => () => request.current?.abort(), []);
  const connection = status.connections.find((c) => c.id === selected);
  const snapshot = snapshots.get(selected);
  async function refresh() {
    if (!connection?.discovery || request.current) return;
    const id = connection.id;
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError('');
    try {
      const result = await api.modelInventory(id, controller.signal);
      if (controller.signal.aborted || request.current !== controller) return;
      if (result.status === 'available')
        setSnapshots((current) => new Map(current).set(id, result));
      else setError(result.message ?? 'Runtime inventory is unavailable.');
    } catch (error) {
      if (!controller.signal.aborted)
        setError(error instanceof Error ? error.message : 'Runtime inventory is unavailable.');
    } finally {
      if (request.current === controller) {
        request.current = null;
        if (!controller.signal.aborted) setBusy(false);
      }
    }
  }
  function select(id: string) {
    request.current?.abort();
    request.current = null;
    setBusy(false);
    setError('');
    setSelected(id);
  }
  return (
    <details className="runtime-models">
      <summary>Runtime models</summary>
      <p>
        Inspect installed models and reported capabilities. Refresh does not load models. Model
        choices above come from your configured profiles.
      </p>
      <p>Chat image input requires a configured model profile with vision enabled.</p>
      <select
        aria-label="Runtime connection"
        value={selected}
        onChange={(event) => select(event.target.value)}
      >
        {status.connections.map((c) => (
          <option key={c.id} value={c.id}>
            {c.label}
          </option>
        ))}
      </select>
      <button type="button" onClick={refresh} disabled={busy || !connection?.discovery}>
        {busy ? 'Reading models…' : 'Refresh models'}
      </button>
      {!connection?.discovery && <p>Models for this connection are configured manually.</p>}
      {error && (
        <p role="alert">
          {error}
          {snapshot && ' Showing the last successful snapshot.'}
        </p>
      )}
      {snapshot && (
        <>
          <p>Snapshot from the last successful refresh; runtime state may have changed.</p>
          {snapshot.models.length === 0 && <p>The runtime reported no chat models.</p>}
          <ul>
            {snapshot.models.map((model) => (
              <li key={model.id}>
                <strong>{model.label}</strong> ·{' '}
                {model.state === 'unknown'
                  ? 'Load state unknown'
                  : model.state === 'sleeping'
                    ? 'Sleeping'
                    : model.state === 'loaded'
                      ? 'Loaded'
                      : 'Not loaded'}
                {model.tool_calls !== null &&
                  ` · Tool support ${model.tool_calls ? 'advertised' : 'not advertised'}`}
                {model.vision !== null &&
                  ` · Vision ${model.vision ? 'advertised' : 'not advertised'}`}
                {model.context_length !== null &&
                  ` · Loaded context ${model.context_length.toLocaleString()} tokens`}
                {model.max_context_length !== null &&
                  ` · Advertised model limit ${model.max_context_length.toLocaleString()} tokens`}
                {!status.model_catalog.profiles.some(
                  (p) => p.backend === selected && p.model === model.id,
                ) && ' · No named profile'}
              </li>
            ))}
          </ul>
        </>
      )}
    </details>
  );
}
