import { useEffect, useRef, useState } from 'react';
import { api, type ImageInventory as Inventory } from './api';

// App keys this surface by preset identity. Switching presets disposes the read
// and its snapshot; runtime information is never persisted or used as authority.
export function ImageInventory({
  id,
  model,
  catalog,
}: {
  id: string;
  model: string;
  catalog: readonly { id: string; file: string; label: string }[];
}) {
  const [snapshot, setSnapshot] = useState<Inventory>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const request = useRef<AbortController | null>(null);
  useEffect(() => () => request.current?.abort(), []);
  async function refresh() {
    if (request.current) return;
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError('');
    try {
      const value = await api.imageInventory(id, model, controller.signal);
      if (!controller.signal.aborted && request.current === controller) setSnapshot(value);
    } catch (error) {
      if (!controller.signal.aborted)
        setError(error instanceof Error ? error.message : 'WanGP discovery failed.');
    } finally {
      if (request.current === controller) {
        request.current = null;
        if (!controller.signal.aborted) setBusy(false);
      }
    }
  }
  return (
    <details className="runtime-models">
      <summary>WanGP models and LoRAs</summary>
      <p>
        Inspect the WanGP catalog and adapters for {model}. Refresh does not install or load
        weights.
      </p>
      <button type="button" disabled={busy} onClick={refresh}>
        {busy ? 'Reading WanGP…' : 'Refresh WanGP'}
      </button>
      {error && (
        <p role="alert">
          {error}
          {snapshot && ' Showing the last successful snapshot.'}
        </p>
      )}
      {snapshot && (
        <>
          <p>
            Checkpoint for {model}:{' '}
            {snapshot.available ? 'Available locally' : 'Missing or incomplete'}. Load state is
            unknown. This snapshot does not guarantee other required weights or enough memory.
          </p>
          <p>Capabilities: {snapshot.capabilities.join(', ') || 'None reported'}</p>
          <p>Local LoRAs for this base model: {snapshot.loras.length || 'None'}</p>
          <ul>
            {snapshot.loras.map((file) => (
              <li key={file}>
                {file}
                {!catalog.some((l) => l.file === file) && ' · Not configured for chat selection'}
              </li>
            ))}
          </ul>
          {catalog
            .filter((l) => !snapshot.loras.includes(l.file))
            .map((l) => (
              <p key={l.id}>{l.label}: configured adapter not found locally.</p>
            ))}
          <details>
            <summary>Image model catalog ({snapshot.models.length})</summary>
            <p>
              These are WanGP model definitions. Local availability was checked only for the
              selected preset; select generation models through configured presets.
            </p>
            <ul>
              {snapshot.models.map((m) => (
                <li key={m.id}>
                  {m.label} · {m.id}
                </li>
              ))}
            </ul>
          </details>
        </>
      )}
    </details>
  );
}
