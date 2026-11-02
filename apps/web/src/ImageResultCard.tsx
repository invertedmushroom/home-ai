import { useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { ImageJob } from './api';

const phaseLabels: Record<NonNullable<ImageJob['progress']>['phase'], string> = {
  loading_model: 'Loading model',
  encoding_text: 'Encoding prompt',
  inference: 'Denoising',
  inference_stage_1: 'Denoising · first pass',
  inference_stage_2: 'Denoising · second pass',
  inference_stage_3: 'Denoising · third pass',
  decoding: 'Decoding image',
  downloading_output: 'Retrieving and validating image',
  cancelled: 'WanGP reported cancellation',
};

export function ImageResultCard({
  job,
  onReconcile,
  onInspect,
  operatorResolution = true,
}: {
  job: ImageJob;
  onReconcile: () => void;
  onInspect: (expanded: boolean) => void;
  operatorResolution?: boolean;
}) {
  const [working, setWorking] = useState(false);
  const [error, setError] = useState('');
  const action = useRef<AbortController | null>(null);
  const [now, setNow] = useState(Date.now);
  const hosted = job.profile.provider === 'openai';
  useEffect(() => () => action.current?.abort(), []);
  useEffect(() => {
    if (job.finished_at_ms !== null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [job.id, job.finished_at_ms]);

  async function reconcile(confirmStopped = false) {
    if (
      confirmStopped &&
      !window.confirm(
        hosted
          ? 'Accept the unknown OpenAI image outcome? The request may have completed and incurred charges. Deep cannot recover its result or cancel it. This unlocks image jobs and workspace deletion without publishing an image.'
          : 'Continue only after you have stopped and checked the WanGP runtime. Deep cannot verify the previous outcome. This unlocks image jobs and workspace deletion without publishing an image.',
      )
    )
      return;
    const controller = new AbortController();
    action.current?.abort();
    action.current = controller;
    setWorking(true);
    setError('');
    try {
      if (confirmStopped) {
        await api.resolveImageOutcome(job, controller.signal);
      } else {
        await api.reconcileImageJob(job, controller.signal);
      }
      if (!controller.signal.aborted) onReconcile();
    } catch (error) {
      if (!controller.signal.aborted) {
        setError(error instanceof Error ? error.message : 'Image reconciliation failed.');
      }
    } finally {
      if (!controller.signal.aborted) setWorking(false);
    }
  }

  const image = job.output_image;
  const live = job.finished_at_ms === null;
  const progress = job.progress;
  const elapsed = Math.floor(Math.max(0, (job.finished_at_ms ?? now) - job.created_at_ms) / 1000);
  const contactAge =
    job.last_contact_at_ms == null
      ? null
      : Math.floor(Math.max(0, now - job.last_contact_at_ms) / 1000);
  return (
    <section className="image-result-card" aria-label="Image generation result">
      {!image && (
        <>
          <strong>Image generation · {job.status}</strong>
          <p>
            {job.profile.label} · {job.profile.model}
          </p>
          <p>Elapsed: {elapsed}s</p>
          {job.input_image && (
            <p>
              Reference image:{' '}
              <a href={`/api/image-assets/${job.input_image.asset.id}/content`}>
                {job.input_image.asset.name ?? job.input_image.asset.id}
              </a>
            </p>
          )}
        </>
      )}
      {live && (
        <div className="image-progress">
          <p>
            {progress ? (
              <>
                {job.status !== 'running' && 'Last reported phase: '}
                {phaseLabels[progress.phase]}
                {progress.current_step !== null &&
                  progress.total_steps !== null &&
                  ` · ${progress.current_step} of ${progress.total_steps} ${progress.phase.startsWith('inference') ? 'steps' : 'units'}`}
              </>
            ) : job.status === 'planned' ? (
              'Checking image runtime'
            ) : job.status === 'submitting' ? (
              hosted ? (
                'Submitting to OpenAI'
              ) : (
                'Submitting to WanGP'
              )
            ) : hosted ? (
              'Awaiting the OpenAI response'
            ) : (
              'Awaiting WanGP progress'
            )}
          </p>
          {job.status === 'running' && (
            <progress
              aria-label="Image generation phase progress"
              max={progress?.total_steps ?? undefined}
              value={
                progress?.total_steps != null && progress.current_step !== null
                  ? progress.current_step
                  : undefined
              }
            />
          )}
          <p className={contactAge !== null && contactAge >= 10 ? 'run-error' : undefined}>
            {hosted
              ? 'OpenAI sends no progress updates; the image arrives when the request completes.'
              : contactAge === null
                ? 'Awaiting WanGP job contact'
                : `WanGP last contacted ${contactAge}s ago${contactAge >= 10 ? ' · Updates delayed' : ''}`}
          </p>
        </div>
      )}
      {job.error && <p className="run-error">{job.error}</p>}
      {image && (
        <div className="generated-image">
          <button
            type="button"
            className="image-open"
            aria-label="Expand image"
            title="Expand image"
            onClick={() => onInspect(true)}
          >
            <img
              src={`/api/image-assets/${image.id}/content`}
              alt={`Generated image using ${job.profile.label}`}
              width={image.width}
              height={image.height}
              onError={() => setError('The saved image could not be loaded. Reload to retry.')}
            />
          </button>
          <div className="image-actions">
            <button
              type="button"
              aria-label="Image job details"
              title="Image job details"
              onClick={() => onInspect(false)}
            >
              ⓘ
            </button>
            <button
              type="button"
              aria-label="Expand generated image"
              title="Expand image"
              onClick={() => onInspect(true)}
            >
              ↗
            </button>
          </div>
        </div>
      )}
      {job.status === 'unresolved' && (
        <>
          <p>Remote completion is unknown. New image jobs and workspace deletion are blocked.</p>
          {job.remote_job_id && (
            <button type="button" disabled={working} onClick={() => void reconcile()}>
              {working ? 'Reconciling…' : 'Reconcile image job'}
            </button>
          )}
          {operatorResolution ? (
            <button type="button" disabled={working} onClick={() => void reconcile(true)}>
              {hosted ? 'Accept unknown OpenAI outcome' : 'Resolve after stopping WanGP'}
            </button>
          ) : (
            <p>
              Ask the operator to check {hosted ? 'OpenAI' : 'WanGP'} and resolve the unknown
              outcome.
            </p>
          )}
        </>
      )}
      {error && (
        <p className="run-error" role="alert">
          {error}
        </p>
      )}
      {!image && (
        <details>
          <summary>Image job details</summary>
          <ImageJobDetails job={job} />
        </details>
      )}
    </section>
  );
}

export function ImageJobDetails({ job }: { job: ImageJob }) {
  const hosted = job.profile.provider === 'openai';
  return (
    <section className="image-job-details" aria-label="Image job details">
      <h2>
        {job.profile.label} · {job.status}
      </h2>
      <p>
        {job.profile.model} ·{' '}
        {Math.floor(Math.max(0, (job.finished_at_ms ?? Date.now()) - job.created_at_ms) / 1000)}s
      </p>
      {job.input_image && (
        <p>
          Reference image:{' '}
          <a href={`/api/image-assets/${job.input_image.asset.id}/content`}>
            {job.input_image.asset.name ?? job.input_image.asset.id}
          </a>
        </p>
      )}
      {job.output_image && (
        <>
          <p>
            {job.output_image.width} × {job.output_image.height} · {job.output_image.media_type} ·{' '}
            {job.output_image.bytes} bytes
          </p>
          <a
            href={`/api/image-assets/${job.output_image.id}/content?download=true`}
            download={`image-${job.output_image.id}.${job.output_image.media_type === 'image/png' ? 'png' : 'jpg'}`}
          >
            Download image
          </a>
        </>
      )}
      <p>
        Job {job.id}
        {!hosted && ` · seed ${job.seed}`}
      </p>
      {!hosted && job.remote_job_id && <p>WanGP job {job.remote_job_id}</p>}
      <pre>{job.prompt}</pre>
      <p>Configured profile fingerprint: {job.profile.fingerprint}</p>
      {!hosted && job.generation && (
        <>
          <p>Applied LoRAs: {job.generation.loras.join(', ') || 'None'}</p>
          {!!job.generation.loras.length && (
            <p>Applied strengths: {job.generation.lora_strengths || 'Runtime defaults'}</p>
          )}
          {!!job.generation.selected_lora_ids.length && (
            <p>Selected adapter IDs: {job.generation.selected_lora_ids.join(', ')}</p>
          )}
        </>
      )}
      {job.input_image && (
        <p>
          Input asset {job.input_image.asset.id} · role: reference · SHA-256{' '}
          {job.input_image.asset.sha256}
        </p>
      )}
      <p>
        Generation returns image metadata to chat. Visual inspection requires attaching an image to
        a message.
      </p>
    </section>
  );
}
