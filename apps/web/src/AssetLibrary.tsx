import { useEffect, useRef, useState } from 'react';
import type { ChangeEvent } from 'react';
import { api, HttpError } from './api';
import type { Asset, ImageAsset, Proposal, Revision } from './api';
import type { AssetChange, ManualEditRequest } from './project';
import { applyAssetChanges, projectSnapshotSchema } from './project';
import { useDownloads } from './useDownloads';

type Pending =
  | { kind: 'edit'; input: ManualEditRequest }
  | { kind: 'text'; input: { base_revision: { id: string | null }; asset_ids: string[] } };
type Upload = { id: string; file: File };
type Props = {
  workspace: string;
  images: ImageAsset[];
  assets: Asset[];
  head?: Revision;
  busy: boolean;
  enabled?: boolean;
  refresh: () => Promise<unknown>;
  onProposal: (proposal: Proposal) => void;
};

export function AssetLibrary({
  workspace,
  images,
  assets,
  head,
  busy,
  enabled,
  refresh,
  onProposal,
}: Props) {
  const [paths, setPaths] = useState<Record<string, string>>({});
  const [pending, setPending] = useState<Pending | null>(null);
  const [upload, setUpload] = useState<Upload | null>(null);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const action = useRef<AbortController | null>(null);
  const active = useRef(false);
  const download = useDownloads();
  useEffect(() => () => action.current?.abort(), []);

  async function uploadImage(input: Upload) {
    if (busy || active.current) return;
    const controller = new AbortController();
    action.current = controller;
    active.current = true;
    setWorking(true);
    setError('');
    setNotice('');
    setUpload(input);
    let saved = false;
    try {
      await api.uploadImage(workspace, input.id, input.file, controller.signal);
      if (controller.signal.aborted) return;
      saved = true;
      setUpload(null);
      setNotice('Image saved to the workspace library.');
      await refresh();
    } catch (error) {
      if (!controller.signal.aborted) {
        setError(
          `${saved ? 'Image saved, but the library could not refresh. ' : ''}${error instanceof Error ? error.message : 'Image upload failed.'}`,
        );
      }
    } finally {
      active.current = false;
      if (!controller.signal.aborted) setWorking(false);
    }
  }

  function chooseImage(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (file) void uploadImage({ id: crypto.randomUUID(), file });
  }

  async function submit(input: Pending) {
    if (busy || active.current) return;
    const controller = new AbortController();
    action.current = controller;
    active.current = true;
    setWorking(true);
    setError('');
    setNotice('');
    setPending(input);
    try {
      const proposal =
        input.kind === 'edit'
          ? await api.proposeEdits(workspace, input.input, controller.signal)
          : await api.proposeAssets(workspace, input.input, controller.signal);
      if (controller.signal.aborted) return;
      setPending(null);
      onProposal(proposal);
      setNotice('Asset proposal saved for review.');
      await refresh();
    } catch (error) {
      if (!controller.signal.aborted) {
        setError(error instanceof Error ? error.message : 'Asset proposal failed.');
        if (error instanceof HttpError && [400, 403, 404, 409, 413, 422].includes(error.status))
          setPending(null);
      }
    } finally {
      active.current = false;
      if (!controller.signal.aborted) setWorking(false);
    }
  }

  function bind(change: AssetChange) {
    if (pending || upload || busy || active.current) return;
    try {
      const bindings = applyAssetChanges(head?.asset_bindings ?? [], [change]);
      projectSnapshotSchema.parse({ files: head?.files ?? [], asset_bindings: bindings });
      void submit({
        kind: 'edit',
        input: {
          id: crypto.randomUUID(),
          base_revision: { id: head?.id ?? null },
          changes: [],
          asset_changes: [change],
        },
      });
    } catch (error) {
      setError(error instanceof Error ? error.message : 'Invalid project asset path.');
    }
  }

  async function downloadText(asset: Asset) {
    if (active.current) return;
    const controller = new AbortController();
    action.current = controller;
    active.current = true;
    setWorking(true);
    setError('');
    try {
      const detail = await api.asset(asset.id, controller.signal);
      if (!controller.signal.aborted)
        download(
          new Blob([detail.content], { type: asset.media_type }),
          asset.name.split('/').at(-1)!,
        );
    } catch (error) {
      if (!controller.signal.aborted)
        setError(error instanceof Error ? error.message : 'Asset download failed.');
    } finally {
      active.current = false;
      if (!controller.signal.aborted) setWorking(false);
    }
  }

  const locked = !enabled || busy || working || !!pending || !!upload;
  return (
    <section className="asset-library" aria-label="Workspace assets">
      <div className="library-overview">
        <div>
          <strong>Project files</strong>
          <p>
            {head?.files.length ?? 0} text files · {head?.asset_bindings?.length ?? 0} image
            bindings · {head ? `Revision ${head.number}` : 'Empty project'}
          </p>
        </div>
      </div>
      <div>
        <h2>Workspace library · {images.length + assets.length}</h2>
        <p>
          Generated, uploaded and imported files live here. Use in project creates a proposal for
          review.
        </p>
        {enabled === false && (
          <p className="run-error">
            Restart the Rust server from this checkout to enable image uploads, project bindings and
            bundle export.
          </p>
        )}
        {enabled === undefined && <p>Loading asset API capabilities…</p>}
        <p>
          Asset proposals target{' '}
          {head ? `accepted revision ${head.number}` : 'the empty accepted project'}, not the
          revision currently being viewed.
        </p>
        <label>
          Upload image
          <input
            type="file"
            accept="image/png,image/jpeg"
            aria-label="Upload image"
            disabled={!enabled || busy || working || !!pending}
            onChange={chooseImage}
          />
        </label>
        {upload && (
          <button type="button" disabled={busy || working} onClick={() => void uploadImage(upload)}>
            Retry image upload
          </button>
        )}
        {pending && (
          <p>
            Asset proposal save is unconfirmed. Retry uses the captured base and exact selection.
            <button type="button" disabled={busy || working} onClick={() => void submit(pending)}>
              Retry asset proposal
            </button>
          </p>
        )}
        {notice && <p role="status">{notice}</p>}
        {error && (
          <p className="run-error" role="alert">
            {error}
          </p>
        )}
        <h3>Images · {images.length}</h3>
        {images.map((image) => {
          const extension = image.media_type === 'image/png' ? 'png' : 'jpg';
          const bound = (head?.asset_bindings ?? []).filter((item) => item.asset_id === image.id);
          const path = paths[image.id] ?? bound[0]?.path ?? `assets/image-${image.id}.${extension}`;
          return (
            <article
              key={image.id}
              className="library-image"
              aria-label={`Image asset ${image.id}`}
            >
              <img
                src={`/api/image-assets/${image.id}/content`}
                alt={image.name ?? 'Workspace image'}
                loading="lazy"
              />
              <div>
                <strong>{image.name ?? `image-${image.id}.${extension}`}</strong>
                <p>
                  {image.width} × {image.height} · {image.bytes} bytes ·{' '}
                  {image.origin ?? 'generation'}
                </p>
                {image.image_job_id && (
                  <p>Produced by image job {image.image_job_id.slice(0, 8)}</p>
                )}
                <label>
                  Project path
                  <input
                    aria-label={`Project path for ${image.id}`}
                    value={path}
                    disabled={locked}
                    onChange={(event) =>
                      setPaths((paths) => ({ ...paths, [image.id]: event.target.value }))
                    }
                  />
                </label>
                <button
                  type="button"
                  disabled={locked}
                  onClick={() => bind({ op: 'bind', path: path.trim(), asset_id: image.id })}
                >
                  Use in project
                </button>
                <a
                  href={`/api/image-assets/${image.id}/content?download=true`}
                  download={`image-${image.id}.${extension}`}
                >
                  Download image
                </a>
                {bound.map((binding) => (
                  <p key={binding.path}>
                    Bound at {binding.path}
                    <button
                      type="button"
                      disabled={locked}
                      onClick={() => bind({ op: 'unbind', path: binding.path })}
                    >
                      Remove binding
                    </button>
                  </p>
                ))}
              </div>
            </article>
          );
        })}
        <h3>Text files · {assets.length}</h3>
        {assets.map((asset) => (
          <article key={asset.id} className="library-text">
            <strong>{asset.name}</strong>
            <span>
              {' '}
              · {asset.media_type} · {asset.bytes} bytes · {asset.origin}
            </span>
            {asset.source && (
              <details>
                <summary>Import source</summary>
                <p>
                  <a
                    href={asset.source.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    referrerPolicy="no-referrer"
                  >
                    {asset.source.url}
                  </a>
                </p>
                {asset.source.final_url !== asset.source.url && (
                  <p>
                    Retrieved from{' '}
                    <a
                      href={asset.source.final_url}
                      target="_blank"
                      rel="noopener noreferrer"
                      referrerPolicy="no-referrer"
                    >
                      {asset.source.final_url}
                    </a>
                  </p>
                )}
                <p>
                  SHA-256: <code>{asset.source.sha256}</code>
                </p>
              </details>
            )}
            <button type="button" disabled={working} onClick={() => void downloadText(asset)}>
              Download text asset
            </button>
            <button
              type="button"
              disabled={locked}
              onClick={() =>
                void submit({
                  kind: 'text',
                  input: { base_revision: { id: head?.id ?? null }, asset_ids: [asset.id] },
                })
              }
            >
              Review in project
            </button>
          </article>
        ))}
        <p>
          Image bindings provide relative paths for app preview/export. Image bytes do not enter
          source-edit context or the Node worker.
        </p>
      </div>
    </section>
  );
}
