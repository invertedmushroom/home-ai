import { z } from 'zod';
import {
  applyChanges,
  applyAssetChanges,
  assetBindingsSchema,
  assetChangeSchema,
  fileChangeSchema,
  filesSchema,
  projectSnapshotSchema,
  sameChanges,
  sameAssetChanges,
  validPath,
} from './project';
import type { AssetBinding, ManualEditRequest, ProjectFile } from './project';

const oneSource = (source: {
  run_id: string | null;
  execution_id: string | null;
  edit_id: string | null;
  asset_ids: string[] | null;
}) =>
  [source.run_id, source.execution_id, source.edit_id, source.asset_ids].filter((id) => id !== null)
    .length === 1;

const validMediaType = (value: string) =>
  value.length >= 3 &&
  value.length <= 128 &&
  /^[a-z0-9][a-z0-9!#$&-^_.+]*\/[a-z0-9][a-z0-9!#$&-^_.+]*$/.test(value);

const importSourceSchema = z.object({
  url: z.string().refine(validSourceUrl),
  final_url: z.string().refine(validSourceUrl),
  sha256: z.string().regex(/^[a-f0-9]{64}$/),
  run_id: z.string().uuid(),
  call_id: z.string().regex(/^[a-zA-Z0-9_-]{1,128}$/),
});

const assetSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    name: z.string().refine(validPath),
    media_type: z.string().refine(validMediaType),
    bytes: z.number().int().min(0).max(1_048_576),
    origin: z.enum(['upload', 'execution', 'import']),
    source: importSourceSchema.nullable().default(null),
    execution_id: z.string().uuid().nullable().default(null),
    created_at_ms: z.number().int().nonnegative(),
  })
  .refine(
    (asset) =>
      (asset.origin === 'execution') === (asset.execution_id !== null) &&
      (asset.origin === 'import') === (asset.source !== null),
  );

const assetDetailSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    name: z.string().refine(validPath),
    media_type: z.string().refine(validMediaType),
    bytes: z.number().int().min(0).max(1_048_576),
    origin: z.enum(['upload', 'execution', 'import']),
    source: importSourceSchema.nullable().default(null),
    execution_id: z.string().uuid().nullable().default(null),
    created_at_ms: z.number().int().nonnegative(),
    content: z.string().refine((text) => new TextEncoder().encode(text).length <= 1_048_576),
  })
  .refine(
    (detail) =>
      (detail.origin === 'execution') === (detail.execution_id !== null) &&
      (detail.origin === 'import') === (detail.source !== null) &&
      new TextEncoder().encode(detail.content).length === detail.bytes,
  );

const assetRefSchema = z.object({
  id: z.string().uuid(),
  name: z.string().refine(validPath),
  media_type: z.string().refine(validMediaType),
  bytes: z.number().int().min(0).max(1_048_576),
  path: z.string().refine(validPath),
});

const countSchema = z.number().int().nonnegative().nullable();
function validSourceUrl(value: string) {
  try {
    const url = new URL(value);
    return (
      value.length <= 2048 &&
      !/[\x00-\x20\x7f]/.test(value) &&
      ['http:', 'https:'].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      !url.port &&
      url.hostname.includes('.') &&
      url.hostname.length <= 253 &&
      !url.hostname.endsWith('.') &&
      !/^[\d.]+$/.test(url.hostname) &&
      !url.hostname.startsWith('[') &&
      !/(?:^|\.)(?:localhost|local|internal|test|invalid|example)$/.test(url.hostname) &&
      url.hostname.split('.').every((part) => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(part))
    );
  } catch {
    return false;
  }
}
const searchReceiptSchema = z
  .object({
    id: z.string().uuid(),
    call_id: z
      .string()
      .min(1)
      .max(128)
      .regex(/^[a-zA-Z0-9_-]+$/),
    query: z
      .string()
      .refine(
        (value) =>
          /[^\p{White_Space}]/u.test(value) &&
          new TextEncoder().encode(value).length <= 512 &&
          !/[\x00-\x1f\x7f]/.test(value) &&
          !/(?:^|\p{White_Space})[!:\\]/u.test(value),
      ),
    engine: z.string().min(1).max(64),
    status: z.enum(['pending', 'succeeded', 'failed', 'cancelled', 'interrupted']),
    elapsed_ms: countSchema,
    error: z.string().max(1024).nullable(),
    sources: z
      .array(
        z.object({
          id: z.string().regex(/^S(?:[1-9]|10)$/),
          title: z
            .string()
            .min(1)
            .refine((value) => [...value].length <= 200),
          url: z.string().refine(validSourceUrl),
          snippet: z.string().refine((value) => [...value].length <= 700),
        }),
      )
      .max(5),
  })
  .refine((receipt) => receipt.status === 'succeeded' || receipt.sources.length === 0);
const statsSchema = z.object({
  reported_model: z.string().min(1).max(256).nullable(),
  finish_reason: z.string().min(1).max(64).nullable(),
  input_tokens: countSchema,
  output_tokens: countSchema,
  total_tokens: countSchema,
  first_token_ms: countSchema,
});
const executionResultSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    revision_id: z.string().uuid().nullable().default(null),
    entry: z.string().refine(validPath),
    action: z.enum(['check', 'run']),
    status: z.enum(['succeeded', 'failed', 'cancelled', 'timed_out', 'interrupted']),
    stdout: z.string().refine((text) => new TextEncoder().encode(text).length <= 8192),
    stderr: z.string().refine((text) => new TextEncoder().encode(text).length <= 8192),
    exit_code: z.number().int().min(0).max(255).nullable(),
    signal: z.string().max(32).nullable(),
    truncated: z.boolean(),
    error: z.string().nullable(),
    logs_shortened: z.boolean(),
    output_assets: z.array(assetSchema).max(64).default([]),
  })
  .refine(
    (result) =>
      (result.status !== 'succeeded' || result.exit_code === 0) &&
      (result.status === 'succeeded' || result.output_assets.length === 0) &&
      (result.revision_id === null || result.output_assets.length === 0) &&
      result.output_assets.every(
        (asset) => asset.workspace_id === result.workspace_id && asset.execution_id === result.id,
      ),
  );
const toolExecutionSchema = z.object({
  id: z.string().uuid(),
  call_id: z
    .string()
    .min(1)
    .max(128)
    .regex(/^[a-zA-Z0-9_-]+$/),
  entry: z.string().refine((path) => validPath(path) && /\.(js|mjs|cjs)$/.test(path)),
  action: z.enum(['check', 'run']),
  code: z
    .string()
    .refine((text) => text.trim().length > 0 && new TextEncoder().encode(text).length <= 1_048_576)
    .nullable()
    .optional()
    .default(null),
  input_assets: z.array(assetRefSchema).max(64).default([]),
  result: executionResultSchema.nullable(),
});
const loraFileSchema = z
  .string()
  .min(1)
  .max(256)
  .refine((v) => !/[\\/:\p{Cc}]/u.test(v) && v !== '.' && v !== '..');
const imageLoraSchema = z.object({
  id: z.string().regex(/^[a-zA-Z0-9_-]{1,64}$/),
  label: z
    .string()
    .refine(
      (v) => v.trim().length > 0 && new TextEncoder().encode(v).length <= 160 && !/\p{Cc}/u.test(v),
    ),
  description: z
    .string()
    .refine(
      (v) =>
        v.trim().length > 0 && new TextEncoder().encode(v).length <= 1024 && !/\p{Cc}/u.test(v),
    ),
  file: loraFileSchema,
  trigger_words: z
    .array(
      z
        .string()
        .refine(
          (v) =>
            v.trim().length > 0 && new TextEncoder().encode(v).length <= 128 && !/\p{Cc}/u.test(v),
        ),
    )
    .max(8),
  strength: z.number().finite().min(-10).max(10),
});
const imageInventorySchema = z
  .object({
    profile_id: z.string().regex(/^[a-zA-Z0-9_-]{1,64}$/),
    model: z.string().regex(/^[a-zA-Z0-9_-]{1,64}$/),
    available: z.boolean(),
    capabilities: z
      .array(
        z
          .string()
          .min(1)
          .max(128)
          .refine((v) => !/\p{Cc}/u.test(v)),
      )
      .max(64),
    models: z
      .array(
        z.object({
          id: z.string().regex(/^[a-zA-Z0-9._-]{1,128}$/),
          label: z
            .string()
            .min(1)
            .refine((v) => new TextEncoder().encode(v).length <= 256 && !/\p{Cc}/u.test(v)),
        }),
      )
      .max(256),
    loras: z.array(loraFileSchema).max(256),
  })
  .refine(
    (i) =>
      new Set(i.models.map((m) => m.id)).size === i.models.length &&
      new Set(i.loras).size === i.loras.length &&
      i.models.some((m) => m.id === i.model),
  );
export type ImageInventory = z.infer<typeof imageInventorySchema>;
const imageProfileSchema = z
  .object({
    provider: z.enum(['wangp', 'openai']).default('wangp'),
    id: z.string().regex(/^[a-zA-Z0-9_-]{1,64}$/),
    label: z
      .string()
      .min(1)
      .refine((value) => new TextEncoder().encode(value).length <= 160 && !/\p{Cc}/u.test(value)),
    model: z.string().regex(/^[a-zA-Z0-9._-]{1,128}$/),
    fingerprint: z.string().regex(/^[a-f0-9]{64}$/),
    loras: z
      .array(
        z
          .string()
          .min(1)
          .max(256)
          .refine((v) => !/[\\/:\p{Cc}]/u.test(v)),
      )
      .max(8)
      .nullable()
      .optional(),
    lora_strengths: z
      .string()
      .max(256)
      .refine((v) => !/\p{Cc}/u.test(v))
      .nullable()
      .optional(),
    lora_catalog: z.array(imageLoraSchema).max(16).optional(),
  })
  .refine(
    (profile) =>
      (profile.provider === 'openai' || /^[a-zA-Z0-9_-]{1,64}$/.test(profile.model)) &&
      (profile.provider !== 'openai' || !profile.lora_catalog?.length) &&
      new Set(profile.lora_catalog?.map((l) => l.id)).size ===
        (profile.lora_catalog?.length ?? 0) &&
      new Set(profile.lora_catalog?.map((l) => l.file)).size ===
        (profile.lora_catalog?.length ?? 0) &&
      (profile.lora_catalog ?? []).every((l) => !profile.loras?.includes(l.file)),
  );
const imageAssetSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    image_job_id: z.string().uuid().nullable(),
    name: z
      .string()
      .min(1)
      .refine(
        (value) => new TextEncoder().encode(value).length <= 160 && !/[\p{Cc}\\/]/u.test(value),
      )
      .optional(),
    origin: z.enum(['upload', 'generation']).optional(),
    media_type: z.enum(['image/png', 'image/jpeg']),
    bytes: z.number().int().min(1).max(8_388_608),
    width: z.number().int().min(1).max(2048),
    height: z.number().int().min(1).max(2048),
    sha256: z.string().regex(/^[a-f0-9]{64}$/),
    created_at_ms: z.number().int().nonnegative(),
  })
  .refine(
    (asset) =>
      (asset.origin === 'upload') === (asset.image_job_id === null) &&
      (asset.origin !== 'upload' || !!asset.name),
  );
const imageInputSchema = z.object({
  role: z.literal('reference'),
  asset: imageAssetSchema,
});
function consistentBindings(bindings: AssetBinding[], images: ImageAsset[]) {
  let bytes = 0;
  return bindings.every((binding) => {
    const image = images.find((image) => image.id === binding.asset_id);
    if (!image) return false;
    bytes += image.bytes;
    const path = binding.path.toLowerCase();
    return (
      bytes <= 33_554_432 &&
      (image.media_type === 'image/png' ? path.endsWith('.png') : /\.(jpg|jpeg)$/.test(path))
    );
  });
}
const imageProgressSchema = z
  .object({
    phase: z.enum([
      'loading_model',
      'encoding_text',
      'inference',
      'inference_stage_1',
      'inference_stage_2',
      'inference_stage_3',
      'decoding',
      'downloading_output',
      'cancelled',
    ]),
    current_step: z.number().int().min(0).max(4_294_967_295).nullable(),
    total_steps: z.number().int().min(1).max(4_294_967_295).nullable(),
  })
  .refine(
    (progress) =>
      progress.current_step === null ||
      progress.total_steps === null ||
      progress.current_step <= progress.total_steps,
  );
const imageJobSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    run_id: z.string().uuid(),
    call_id: z.string().regex(/^[a-zA-Z0-9_-]{1,128}$/),
    prompt: z
      .string()
      .refine(
        (value) =>
          value.trim().length > 0 &&
          !value.includes('\0') &&
          new TextEncoder().encode(value).length <= 4096,
      ),
    seed: z.number().int().min(0).max(4_294_967_295),
    status: z.enum([
      'planned',
      'submitting',
      'running',
      'cancelling',
      'unresolved',
      'succeeded',
      'failed',
      'cancelled',
      'interrupted',
    ]),
    error: z
      .string()
      .min(1)
      .refine((value) => new TextEncoder().encode(value).length <= 1024)
      .nullable(),
    created_at_ms: z.number().int().nonnegative(),
    finished_at_ms: z.number().int().nonnegative().nullable(),
    profile: imageProfileSchema,
    generation: z
      .object({
        loras: z.array(loraFileSchema).max(8),
        lora_strengths: z
          .string()
          .max(512)
          .refine((v) => !/\p{Cc}/u.test(v)),
        selected_lora_ids: z.array(z.string().regex(/^[a-zA-Z0-9_-]{1,64}$/)).max(8),
      })
      .optional(),
    output_image: imageAssetSchema.nullable(),
    input_image: imageInputSchema.nullable().default(null),
    remote_job_id: z
      .string()
      .regex(/^[a-zA-Z0-9_:-]{1,128}$/)
      .nullable(),
    progress: imageProgressSchema.nullable().optional(),
    last_contact_at_ms: z
      .number()
      .int()
      .nonnegative()
      .max(Number.MAX_SAFE_INTEGER)
      .nullable()
      .optional(),
  })
  .refine((job) => {
    if (job.generation) {
      const { loras, selected_lora_ids: ids } = job.generation;
      const selected = loras.flatMap((file) =>
        (job.profile.lora_catalog ?? []).filter((l) => l.file === file),
      );
      if (
        new Set(ids).size !== ids.length ||
        selected.length !== ids.length ||
        selected.some((l, index) => ids[index] !== l.id) ||
        (job.profile.loras &&
          (loras.length !== job.profile.loras.length + selected.length ||
            [...job.profile.loras, ...selected.map((l) => l.file)].some(
              (file, index) => loras[index] !== file,
            )))
      )
        return false;
    }
    const success = job.status === 'succeeded';
    const terminal = ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(job.status);
    return (
      success === (job.output_image !== null) &&
      (!job.input_image || job.input_image.asset.workspace_id === job.workspace_id) &&
      terminal === (job.finished_at_ms !== null) &&
      (!terminal || success || job.error !== null) &&
      (!success || job.error === null) &&
      (job.status !== 'unresolved' || job.error !== null) &&
      (job.progress == null || job.last_contact_at_ms != null) &&
      ((job.progress == null && job.last_contact_at_ms == null) || job.remote_job_id !== null) &&
      (!job.output_image ||
        (job.output_image.image_job_id === job.id &&
          job.output_image.workspace_id === job.workspace_id))
    );
  });
const profileIdSchema = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);
const taskSchema = z.enum(['conversation', 'coding']);
const modelRouteSchema = z
  .object({
    profile_id: profileIdSchema,
    label: z.string().min(1).max(128),
    backend: profileIdSchema,
    model: z.string().min(1).max(512),
    task: taskSchema.nullable(),
    vision: z.boolean().default(false),
    automatic: z.boolean(),
    reason: z.string().min(1).max(256),
  })
  .refine((route) => route.automatic === (route.task !== null));
const runSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    status: z.enum(['queued', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted']),
    local_model: z
      .object({
        phase: z.enum(['waiting', 'working', 'draining', 'recovering', 'blocked', 'settled']),
        round: z.number().int().min(1).max(16),
        operation_id: z.string().uuid().nullable(),
      })
      .refine((activity) => (activity.phase === 'waiting') === (activity.operation_id === null))
      .nullable()
      .default(null),
    error: z.string().nullable(),
    provider: z.string(),
    model: z.string(),
    model_route: modelRouteSchema.nullable().optional(),
    output: z.string(),
    stats: statsSchema,
    queue_ms: countSchema,
    duration_ms: countSchema,
    mode: z.enum(['chat', 'app', 'files']),
    workspace_tools: z.boolean().default(false),
    base_revision_id: z.string().uuid().nullable(),
    execution_result: executionResultSchema.nullable().default(null),
    execution_revision_id: z.string().uuid().nullable().default(null),
    node_scope: z.enum(['off', 'revision', 'compute']).optional(),
    input_assets: z.array(assetRefSchema).max(64).default([]),
    tool_execution: toolExecutionSchema.nullable().default(null),
    tool_executions: z.array(toolExecutionSchema).max(2).optional(),
    search_enabled: z.boolean().default(false),
    import_enabled: z.boolean().default(false),
    imports: z.array(assetSchema).max(2).default([]),
    searches: z.array(searchReceiptSchema).max(2).default([]),
    model_rounds: z.number().int().min(0).max(16).default(0),
    image_profile: imageProfileSchema.nullable().optional(),
    image_input: imageInputSchema.nullable().default(null),
    image_job: imageJobSchema.nullable().optional(),
    image_attachments: z.array(imageAssetSchema).max(4).default([]),
  })
  .transform((run) => {
    const node_scope = run.node_scope ?? (run.execution_revision_id ? 'revision' : 'off');
    const tool_executions = run.tool_executions ?? (run.tool_execution ? [run.tool_execution] : []);
    return {
      ...run,
      node_scope,
      tool_executions,
      tool_execution: run.tool_execution ?? tool_executions.at(-1) ?? null,
    };
  })
  .refine(
    (run) =>
      !run.model_route ||
      (run.model_route.backend === run.provider && run.model_route.model === run.model),
  )
  .refine(
    (run) =>
      !run.execution_result ||
      (run.execution_result.workspace_id === run.workspace_id &&
        run.execution_result.revision_id !== null),
  )
  .refine((run) => {
    const scope = run.node_scope;
    const executions = run.tool_executions;
    const scopeValid =
      scope === 'off'
        ? !run.execution_revision_id &&
          !run.input_assets.length &&
          !executions.length &&
          !run.tool_execution
        : scope === 'revision'
          ? !!run.execution_revision_id &&
            !run.input_assets.length &&
            executions.length <= 1 &&
            executions.every((item) => item.code === null && !item.input_assets.length)
          : !run.execution_revision_id &&
            executions.length <= 2 &&
            executions.every(
              (item) =>
                item.entry === '__compute.cjs' && item.action === 'run' && item.code !== null,
            );
    const lastMatch =
      (executions.at(-1)?.id ?? null) === (run.tool_execution?.id ?? null) &&
      (executions.at(-1)?.call_id ?? null) === (run.tool_execution?.call_id ?? null);
    const maxRounds = run.workspace_tools
      ? 16
      : run.image_profile && !run.search_enabled && !run.import_enabled && scope === 'off'
        ? 2
        : run.import_enabled || scope === 'compute'
          ? 5
          : scope === 'revision'
            ? 4
            : 3;
    return (
      scopeValid &&
      run.model_rounds <= maxRounds &&
      (!run.workspace_tools || (run.mode === 'chat' && run.provider !== 'demo')) &&
      lastMatch &&
      (run.import_enabled || !run.imports.length) &&
      new Set(run.imports.map((a) => a.id)).size === run.imports.length &&
      new Set(run.imports.map((a) => a.source?.call_id)).size === run.imports.length &&
      run.imports.every(
        (a) =>
          a.origin === 'import' &&
          a.workspace_id === run.workspace_id &&
          a.source?.run_id === run.id &&
          !run.searches.some((s) => s.call_id === a.source?.call_id) &&
          !executions.some((e) => e.call_id === a.source?.call_id),
      ) &&
      (run.search_enabled || !run.searches.length) &&
      (run.workspace_tools ||
        run.search_enabled ||
        run.import_enabled ||
        scope !== 'off' ||
        run.image_profile ||
        run.model_rounds === 0) &&
      (!run.image_input ||
        (!!run.image_profile && run.image_input.asset.workspace_id === run.workspace_id)) &&
      (!run.image_attachments.length ||
        (run.provider !== 'demo' && run.model_route?.vision === true)) &&
      new Set(run.image_attachments.map((image) => image.id)).size ===
        run.image_attachments.length &&
      run.image_attachments.every((image) => image.workspace_id === run.workspace_id) &&
      run.image_attachments.reduce((total, image) => total + image.bytes, 0) <= 16 * 1024 * 1024 &&
      (!run.image_profile || run.provider !== 'demo') &&
      (!run.image_job ||
        (!!run.image_profile &&
          run.image_job.run_id === run.id &&
          run.image_job.workspace_id === run.workspace_id &&
          JSON.stringify(run.image_job.profile) === JSON.stringify(run.image_profile) &&
          JSON.stringify(run.image_job.input_image) === JSON.stringify(run.image_input) &&
          (run.status !== 'succeeded' ||
            ['succeeded', 'failed'].includes(run.image_job.status)))) &&
      new Set(executions.map((item) => item.id)).size === executions.length &&
      new Set(executions.map((item) => item.call_id)).size === executions.length &&
      executions.every(
        (item) =>
          !run.searches.some((s) => s.call_id === item.call_id) &&
          (run.status !== 'succeeded' || item.result !== null) &&
          (!item.result ||
            (item.result.id === item.id &&
              item.result.workspace_id === run.workspace_id &&
              item.result.revision_id ===
                (scope === 'revision' ? run.execution_revision_id : null) &&
              item.result.entry === item.entry &&
              item.result.action === item.action)),
      ) &&
      new Set(run.searches.map((item) => item.call_id)).size === run.searches.length &&
      run.searches
        .flatMap((item) => item.sources)
        .every((source, index) => source.id === `S${index + 1}`) &&
      (['queued', 'running'].includes(run.status) ||
        run.searches.every((item) => item.status !== 'pending'))
    );
  });
const workspaceSchema = z.object({ id: z.string().uuid(), title: z.string().min(1) });
const revisionSchema = z
  .object({
    id: z.string().uuid(),
    run_id: z.string().uuid().nullable(),
    execution_id: z.string().uuid().nullable(),
    edit_id: z.string().uuid().nullable().default(null),
    asset_ids: z.array(z.string().uuid()).min(1).max(64).nullable().default(null),
    number: z.number().int().positive(),
    html: z.string(),
    files: filesSchema,
    asset_bindings: assetBindingsSchema.optional(),
  })
  .refine(oneSource);
const proposalSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    run_id: z.string().uuid().nullable(),
    execution_id: z.string().uuid().nullable(),
    edit_id: z.string().uuid().nullable().default(null),
    asset_ids: z.array(z.string().uuid()).min(1).max(64).nullable().default(null),
    base_revision_id: z.string().uuid().nullable(),
    status: z.enum(['pending', 'applied', 'rejected']),
    changes: z.array(fileChangeSchema).max(64),
    asset_changes: z.array(assetChangeSchema).max(64).optional(),
    applied_revision_id: z.string().uuid().nullable(),
    rejection_reason: z
      .string()
      .refine((value) => new TextEncoder().encode(value).length <= 4096)
      .nullable()
      .default(null),
  })
  .refine(oneSource)
  .refine((proposal) => {
    const paths = [...proposal.changes, ...(proposal.asset_changes ?? [])].map((item) =>
      item.path.toLowerCase(),
    );
    return (
      paths.length >= 1 &&
      paths.length <= 64 &&
      new Set(paths).size === paths.length &&
      (!proposal.asset_changes?.length || proposal.edit_id !== null)
    );
  })
  .refine((proposal) => proposal.rejection_reason === null || proposal.status === 'rejected');
const detailSchema = z
  .object({
    workspace: workspaceSchema,
    messages: z.array(
      z.object({
        role: z.enum(['user', 'assistant']),
        content: z.string(),
        run_id: z.string().uuid(),
      }),
    ),
    revisions: z.array(revisionSchema),
    assets: z.array(assetSchema).default([]),
    image_assets: z.array(imageAssetSchema).optional(),
    runs: z.array(runSchema),
    proposals: z.array(proposalSchema),
  })
  .refine(
    (detail) =>
      detail.assets.every((asset) => asset.workspace_id === detail.workspace.id) &&
      new Set((detail.image_assets ?? []).map((image) => image.id)).size ===
        (detail.image_assets ?? []).length &&
      (detail.image_assets ?? []).every(
        (image) =>
          image.workspace_id === detail.workspace.id &&
          (image.origin === 'upload' ||
            detail.runs.some(
              (run) =>
                run.image_job?.id === image.image_job_id &&
                run.image_job.output_image?.id === image.id &&
                JSON.stringify(run.image_job.output_image) === JSON.stringify(image),
            )),
      ) &&
      detail.revisions.every(
        (revision) =>
          projectSnapshotSchema.safeParse({
            files: revision.files,
            asset_bindings: revision.asset_bindings ?? [],
          }).success &&
          consistentBindings(revision.asset_bindings ?? [], detail.image_assets ?? []),
      ) &&
      detail.runs.every(
        (run) =>
          run.workspace_id === detail.workspace.id &&
          run.image_attachments.every((attached) =>
            detail.image_assets?.some(
              (image) => JSON.stringify(image) === JSON.stringify(attached),
            ),
          ) &&
          (!run.image_input ||
            detail.image_assets?.some(
              (image) => JSON.stringify(image) === JSON.stringify(run.image_input?.asset),
            )) &&
          (!run.image_job?.output_image ||
            detail.image_assets?.some((image) => image.id === run.image_job?.output_image?.id)) &&
          (!run.execution_result ||
            detail.revisions.some((revision) => revision.id === run.execution_result!.revision_id)),
      ) &&
      detail.proposals.every((proposal) => {
        if (
          proposal.workspace_id !== detail.workspace.id ||
          (proposal.run_id !== null && !detail.runs.some((run) => run.id === proposal.run_id)) ||
          (proposal.asset_ids !== null &&
            detail.assets.length > 0 &&
            !proposal.asset_ids.every((id) => detail.assets.some((asset) => asset.id === id)))
        )
          return false;
        const base = detail.revisions.find((revision) => revision.id === proposal.base_revision_id);
        if (proposal.base_revision_id !== null && !base) return false;
        if ((proposal.status === 'applied') !== (proposal.applied_revision_id !== null))
          return false;
        if (
          proposal.applied_revision_id &&
          !detail.revisions.some((revision) => revision.id === proposal.applied_revision_id)
        )
          return false;
        try {
          const files = applyChanges(base?.files ?? [], proposal.changes);
          const bindings = applyAssetChanges(
            base?.asset_bindings ?? [],
            proposal.asset_changes ?? [],
          );
          projectSnapshotSchema.parse({ files, asset_bindings: bindings });
          if (!consistentBindings(bindings, detail.image_assets ?? [])) return false;
          const applied = detail.revisions.find(
            (revision) => revision.id === proposal.applied_revision_id,
          );
          return (
            !applied ||
            (applied.run_id === proposal.run_id &&
              applied.execution_id === proposal.execution_id &&
              applied.edit_id === proposal.edit_id &&
              JSON.stringify(applied.asset_ids) === JSON.stringify(proposal.asset_ids) &&
              JSON.stringify(applied.asset_bindings ?? []) === JSON.stringify(bindings) &&
              applied.files.length === files.length &&
              files.every(
                (file, index) =>
                  file.path === applied.files[index].path &&
                  file.content === applied.files[index].content,
              ))
          );
        } catch {
          return false;
        }
      }),
    'Proposal evidence is inconsistent.',
  );
const backendIdSchema = profileIdSchema;
const backendKindSchema = z.enum(['demo', 'compatible', 'lmstudio', 'groq', 'openai']);
const connectionSchema = z.object({
  id: profileIdSchema,
  label: z.string().min(1).max(128),
  kind: backendKindSchema,
  discovery: z.boolean(),
});
const modelCatalogSchema = z
  .object({
    profiles: z
      .array(
        z
          .object({
            id: profileIdSchema,
            label: z.string().min(1).max(128),
            backend: profileIdSchema,
            model: z.string().min(1).max(512),
            tool_calls: z.boolean(),
            vision: z.boolean().default(false),
          })
          .strict(),
      )
      .max(64),
    preferences: z
      .object({
        conversation: z.array(profileIdSchema).max(64),
        coding: z.array(profileIdSchema).max(64),
      })
      .strict(),
    default_profile: profileIdSchema.nullable().default(null),
  })
  .strict();
const statusSchema = z
  .object({
    provider: backendIdSchema,
    model: z.string(),
    model_health: z.literal('not_probed'),
    guest_mode: z.boolean().default(false),
    search_available: z.boolean().default(false),
    url_import_available: z.boolean().default(false),
    execution_available: z.boolean().default(false),
    asset_library_available: z.boolean().default(false),
    image_profiles: z.array(imageProfileSchema).default([]),
    image_input_available: z.boolean().default(false),
    chat_image_input_available: z.boolean().default(false),
    backends: z
      .array(
        z.object({
          id: backendIdSchema,
          model: z.string().min(1),
          kind: backendKindSchema.optional(),
          profile_required: z.boolean().default(false),
        }),
      )
      .min(1),
    connections: z.array(connectionSchema).max(21).default([]),
    model_discovery_available: z.boolean().default(false),
    model_catalog: modelCatalogSchema.default({
      profiles: [],
      preferences: { conversation: [], coding: [] },
      default_profile: null,
    }),
  })
  .refine(
    (status) =>
      new Set(status.backends.map((backend) => backend.id)).size === status.backends.length &&
      new Set(status.image_profiles.map((profile) => profile.id)).size ===
        status.image_profiles.length &&
      status.backends.some(
        (backend) => backend.id === status.provider && backend.model === status.model,
      ) &&
      new Set(status.model_catalog.profiles.map((p) => p.id)).size ===
        status.model_catalog.profiles.length &&
      status.model_catalog.profiles.every(
        (p) =>
          p.backend !== 'demo' &&
          (status.backends.some((b) => b.id === p.backend) ||
            status.connections.some((c) => c.id === p.backend)),
      ) &&
      new Set(status.connections.map((c) => c.id)).size === status.connections.length &&
      (!status.model_catalog.default_profile ||
        status.model_catalog.profiles.some(
          (p) =>
            p.id === status.model_catalog.default_profile &&
            p.backend === status.provider &&
            p.model === status.model,
        )) &&
      Object.values(status.model_catalog.preferences).every(
        (ids) =>
          new Set(ids).size === ids.length &&
          ids.every((id) =>
            status.model_catalog.profiles.some(
              (p) =>
                p.id === id &&
                ['compatible', 'lmstudio'].includes(
                  status.connections.find((c) => c.id === p.backend)?.kind ??
                    status.backends.find((b) => b.id === p.backend)?.kind ??
                    p.backend,
                ),
            ),
          ),
      ),
    'Backend configuration is inconsistent.',
  );

export type Asset = z.infer<typeof assetSchema>;
export type ImageJob = z.infer<typeof imageJobSchema>;
export type ImageAsset = z.infer<typeof imageAssetSchema>;
export type AssetDetail = z.infer<typeof assetDetailSchema>;
export type AssetRef = z.infer<typeof assetRefSchema>;
export type ToolExecution = z.infer<typeof toolExecutionSchema>;
export type Run = z.infer<typeof runSchema>;
export type ExecutionAttachment = z.infer<typeof executionResultSchema>;
export type Workspace = z.infer<typeof workspaceSchema>;
export type Revision = z.infer<typeof revisionSchema>;
export type Proposal = z.infer<typeof proposalSchema>;
export type WorkspaceDetail = z.infer<typeof detailSchema>;
export type ServiceStatus = z.infer<typeof statusSchema>;
const modelInventorySchema = z
  .object({
    connection_id: profileIdSchema,
    status: z.enum(['available', 'unavailable', 'unsupported']),
    message: z.string().min(1).max(512).nullable(),
    models: z
      .array(
        z.object({
          id: z.string().min(1).max(512),
          label: z.string().min(1).max(512),
          state: z.enum(['loaded', 'unloaded', 'sleeping', 'unknown']),
          tool_calls: z.boolean().nullable(),
          vision: z.boolean().nullable(),
          context_length: z.number().int().positive().safe().nullable(),
          max_context_length: z.number().int().positive().safe().nullable(),
        }),
      )
      .max(256),
  })
  .refine(
    (inventory) =>
      new Set(inventory.models.map((m) => m.id)).size === inventory.models.length &&
      (inventory.status === 'available'
        ? inventory.message === null
        : inventory.models.length === 0 && inventory.message !== null),
  );
export type ModelInventory = z.infer<typeof modelInventorySchema>;

const workerSchema = z
  .object({ enabled: z.boolean(), ready: z.boolean(), message: z.string() })
  .refine((value) => !value.ready || value.enabled);
const executionSchema = z
  .object({
    id: z.string().uuid(),
    workspace_id: z.string().uuid(),
    revision_id: z.string().uuid().nullable(),
    entry: z.string(),
    action: z.enum(['check', 'run']),
    code: z.string().nullable().optional().default(null),
    input_assets: z.array(assetRefSchema).max(64).default([]),
    image_id: z.string().regex(/^sha256:[a-f0-9]{64}$/),
    status: z.enum([
      'queued',
      'running',
      'cancelling',
      'succeeded',
      'failed',
      'cancelled',
      'timed_out',
      'interrupted',
    ]),
    stdout: z.string().refine((text) => new TextEncoder().encode(text).length <= 65536),
    stderr: z.string().refine((text) => new TextEncoder().encode(text).length <= 65536),
    exit_code: z.number().int().min(0).max(255).nullable(),
    signal: z.string().max(32).nullable(),
    truncated: z.boolean(),
    error: z.string().nullable(),
    created_at_ms: z.number().int().nonnegative(),
    finished_at_ms: z.number().int().nonnegative().nullable(),
    outputs: z
      .array(
        z.object({ path: z.string().refine(validPath), bytes: z.number().int().nonnegative() }),
      )
      .max(64)
      .refine(
        (files) =>
          new Set(files.map((file) => file.path.toLowerCase())).size === files.length &&
          files.reduce((total, file) => total + file.path.length + file.bytes, 0) <= 1048576,
      ),
    output_assets: z.array(assetSchema).max(64).default([]),
  })
  .refine(
    (job) =>
      (job.revision_id !== null) !== (job.code !== null) &&
      ['queued', 'running', 'cancelling'].includes(job.status) === (job.finished_at_ms === null) &&
      (job.status !== 'succeeded' || job.exit_code === 0) &&
      (job.status === 'succeeded' || (job.outputs.length === 0 && job.output_assets.length === 0)),
    'Inconsistent execution result.',
  );
export type Execution = z.infer<typeof executionSchema>;
export type WorkerStatus = z.infer<typeof workerSchema>;
export type ExecutionRequest = {
  id: string;
  revision_id: string;
  entry: string;
  action: 'check' | 'run';
};
export type NodeScope = 'off' | 'revision' | 'compute';
export type RunRequest = {
  workspace_tools?: boolean;
  prompt: string;
  mode: string;
  backend?: string;
  model_selection?:
    { mode: 'auto'; task: 'conversation' | 'coding' } | { mode: 'profile'; profile_id: string };
  base_revision?: { id: string | null };
  execution_result_id?: string;
  search?: boolean;
  allow_url_import?: boolean;
  execution_revision_id?: string;
  node_scope?: NodeScope;
  input_asset_ids?: string[];
  image_profile_id?: string;
  image_input?: { role: 'reference'; asset_id: string };
  image_attachment_ids?: string[];
};
export const imagePending = (job: ImageJob | null | undefined) =>
  !!job && ['planned', 'submitting', 'running', 'cancelling'].includes(job.status);
export const executing = (job: Execution) =>
  ['queued', 'running', 'cancelling'].includes(job.status);

export const pending = (run: Run) => run.status === 'queued' || run.status === 'running';
export const modelCleanupPending = (run: Run) =>
  run.local_model?.phase === 'draining' || run.local_model?.phase === 'recovering';

export class HttpError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

async function responseError(response: Response) {
  const text = await response.text();
  let message = `Request failed (${response.status}).`;
  try {
    const parsed = z.object({ error: z.string() }).safeParse(JSON.parse(text));
    if (parsed.success) message = parsed.data.error;
  } catch {
    /* Non-JSON rejection. */
  }
  if (message === 'API route not found.' || response.status === 405)
    message =
      'This API server does not support this action. Restart the Rust server from this checkout, then retry. Your current files and draft are preserved.';
  return new HttpError(message, response.status);
}

async function accepted<T>(response: Response, schema: z.ZodType<T>): Promise<T> {
  if (!response.ok) {
    throw await responseError(response);
  }
  if (response.status === 204) return schema.parse(undefined);
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new Error('The server returned invalid JSON. Check the API address.');
  }
  const parsed = schema.safeParse(value);
  if (!parsed.success) {
    const mismatch = (issue: z.core.$ZodIssue) => {
      const detail =
        issue.message.length > 120 ? `${issue.message.slice(0, 117)}...` : issue.message;
      return `${issue.path.join('.') || 'response'}: ${detail}`;
    };
    const issues = parsed.error.issues.slice(0, 3).map(mismatch).join('; ');
    const rest = parsed.error.issues.length - Math.min(3, parsed.error.issues.length);
    throw new Error(
      `The server response has an incompatible shape. Refresh or check the server version.` +
        (issues ? ` First mismatch — ${issues}${rest > 0 ? ` (+${rest} more).` : '.'}` : '.'),
    );
  }
  return parsed.data;
}

async function request<T>(
  path: string,
  schema: z.ZodType<T>,
  method = 'GET',
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  return accepted(
    await fetch(`/api${path}`, {
      method,
      signal,
      headers: {
        'X-Deep-Request': '1',
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
    schema,
  );
}

async function binary(
  path: string,
  type: string,
  limit: number,
  signal?: AbortSignal,
  body?: unknown,
) {
  const response = await fetch(`/api${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
  });
  if (!response.ok) throw await responseError(response);
  if (response.headers.get('content-type')?.split(';')[0] !== type || !response.body) {
    await response.body?.cancel();
    throw new Error('The server returned an invalid binary response.');
  }
  const reader = response.body.getReader();
  const chunks: BlobPart[] = [];
  let bytes = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > limit) {
        await reader.cancel();
        throw new Error('Binary response exceeded its byte limit.');
      }
      chunks.push(new Uint8Array(value).buffer);
    }
  } finally {
    reader.releaseLock();
  }
  return new Blob(chunks, { type });
}

async function sha256(bytes: ArrayBuffer) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))]
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('');
}

export const api = {
  imageInventory: (profile: string, model: string, signal?: AbortSignal) =>
    request(
      `/image-profiles/${encodeURIComponent(profile)}/inventory`,
      imageInventorySchema.refine((i) => i.profile_id === profile && i.model === model),
      'GET',
      undefined,
      signal,
    ),
  modelInventory: (connection: string, signal?: AbortSignal) =>
    request(
      `/connections/${encodeURIComponent(connection)}/models`,
      modelInventorySchema.refine((i) => i.connection_id === connection),
      'GET',
      undefined,
      signal,
    ),
  initialize: async (signal: AbortSignal) => {
    const start = () =>
      request('/session', z.object({ guest_mode: z.boolean() }), 'POST', undefined, signal);
    if (navigator.locks) {
      return navigator.locks.request('deep-guest-session', { signal }, start);
    } else {
      const session = await start();
      if (session.guest_mode)
        throw new Error(
          'Guest mode requires a browser with Web Locks support. Use a current browser over HTTPS.',
        );
      return session;
    }
  },
  uploadImage: async (workspace: string, id: string, file: File, signal?: AbortSignal) => {
    if (!['image/png', 'image/jpeg'].includes(file.type) || file.size < 1 || file.size > 8_388_608)
      throw new Error('Choose a PNG or JPEG image of at most 8 MiB.');
    const digest = await sha256(await file.arrayBuffer());
    signal?.throwIfAborted();
    const response = await fetch(
      `/api/workspaces/${workspace}/image-assets?${new URLSearchParams({ id, name: file.name })}`,
      {
        method: 'POST',
        headers: { 'Content-Type': file.type },
        body: file,
        signal,
      },
    );
    return accepted(
      response,
      imageAssetSchema.refine(
        (asset) =>
          asset.id === id &&
          asset.workspace_id === workspace &&
          asset.origin === 'upload' &&
          asset.name === file.name &&
          asset.media_type === file.type &&
          asset.bytes === file.size &&
          asset.sha256 === digest,
      ),
    );
  },
  imageBlob: async (image: ImageAsset, signal?: AbortSignal) => {
    const blob = await binary(
      `/image-assets/${image.id}/content`,
      image.media_type,
      image.bytes,
      signal,
    );
    if (blob.size !== image.bytes || (await sha256(await blob.arrayBuffer())) !== image.sha256)
      throw new Error('Saved image bytes did not match their metadata.');
    return blob;
  },
  exportProject: (
    workspace: string,
    files: ProjectFile[],
    asset_bindings: AssetBinding[],
    signal?: AbortSignal,
  ) =>
    binary(
      `/workspaces/${workspace}/project-export`,
      'application/zip',
      34_668_544,
      signal,
      projectSnapshotSchema.parse({ files, asset_bindings }),
    ),
  resolveImageOutcome: (job: ImageJob, signal?: AbortSignal) =>
    request(
      `/image-jobs/${job.id}/${job.profile.provider === 'openai' ? 'accept-unknown' : 'confirm-stopped'}`,
      imageJobSchema.refine(
        (result) =>
          result.id === job.id &&
          result.workspace_id === job.workspace_id &&
          result.run_id === job.run_id &&
          result.status === 'interrupted' &&
          result.output_image === null,
      ),
      'POST',
      job.profile.provider === 'openai'
        ? { accept_unknown_outcome: true }
        : { confirm_remote_stopped: true },
      signal,
    ),
  reconcileImageJob: (job: ImageJob, signal?: AbortSignal) =>
    request(
      `/image-jobs/${job.id}/reconcile`,
      imageJobSchema.refine(
        (result) =>
          result.id === job.id &&
          result.workspace_id === job.workspace_id &&
          result.run_id === job.run_id &&
          result.profile.fingerprint === job.profile.fingerprint,
      ),
      'POST',
      undefined,
      signal,
    ),
  uploadAsset: (
    workspace: string,
    input: { name: string; content: string; media_type?: string },
    signal?: AbortSignal,
  ) =>
    request(
      `/workspaces/${workspace}/assets`,
      assetSchema.refine(
        (asset) =>
          asset.workspace_id === workspace &&
          asset.name === input.name &&
          asset.origin === 'upload',
      ),
      'POST',
      input,
      signal,
    ),
  asset: (id: string, signal?: AbortSignal) =>
    request(
      `/assets/${id}`,
      assetDetailSchema.refine((asset) => asset.id === id),
      'GET',
      undefined,
      signal,
    ),
  proposeAssets: (
    workspace: string,
    input: { base_revision: { id: string | null }; asset_ids: string[] },
    signal?: AbortSignal,
  ) =>
    request(
      `/workspaces/${workspace}/asset-proposals`,
      proposalSchema.refine(
        (proposal) =>
          proposal.workspace_id === workspace &&
          proposal.base_revision_id === input.base_revision.id &&
          JSON.stringify(proposal.asset_ids) === JSON.stringify(input.asset_ids),
      ),
      'POST',
      input,
      signal,
    ),
  proposeEdits: (workspace: string, input: ManualEditRequest, signal?: AbortSignal) =>
    request(
      `/workspaces/${workspace}/proposals`,
      proposalSchema.refine(
        (proposal) =>
          proposal.workspace_id === workspace &&
          proposal.edit_id === input.id &&
          proposal.base_revision_id === input.base_revision.id &&
          sameChanges(proposal.changes, input.changes) &&
          sameAssetChanges(proposal.asset_changes, input.asset_changes),
      ),
      'POST',
      input,
      signal,
    ),
  proposeOutputs: (job: Execution, signal?: AbortSignal) =>
    request(
      `/executions/${job.id}/proposal`,
      proposalSchema.refine(
        (proposal) =>
          proposal.workspace_id === job.workspace_id &&
          proposal.execution_id === job.id &&
          proposal.base_revision_id === job.revision_id,
      ),
      'POST',
      undefined,
      signal,
    ),
  worker: (signal?: AbortSignal) => request('/worker', workerSchema, 'GET', undefined, signal),
  executions: (workspace: string, signal?: AbortSignal) =>
    request(
      `/workspaces/${workspace}/executions`,
      z
        .array(executionSchema)
        .refine((jobs) => jobs.every((job) => job.workspace_id === workspace)),
      'GET',
      undefined,
      signal,
    ),
  execute: (workspace: string, input: ExecutionRequest, signal?: AbortSignal) =>
    request(
      `/workspaces/${workspace}/executions`,
      executionSchema.refine(
        (job) =>
          job.id === input.id &&
          job.workspace_id === workspace &&
          job.revision_id === input.revision_id &&
          job.entry === input.entry &&
          job.action === input.action,
      ),
      'POST',
      input,
      signal,
    ),
  cancelExecution: (id: string, signal?: AbortSignal) =>
    request(
      `/executions/${id}`,
      executionSchema.refine((job) => job.id === id),
      'DELETE',
      undefined,
      signal,
    ),
  status: (signal?: AbortSignal) => request('/status', statusSchema, 'GET', undefined, signal),
  workspaces: (signal?: AbortSignal) =>
    request('/workspaces', z.array(workspaceSchema), 'GET', undefined, signal),
  workspace: (id: string, signal?: AbortSignal) =>
    request(`/workspaces/${id}`, detailSchema, 'GET', undefined, signal),
  createWorkspace: (title: string) => request('/workspaces', workspaceSchema, 'POST', { title }),
  branchWorkspace: (
    id: string,
    input: { title?: string; up_to_run_id?: string } = {},
    signal?: AbortSignal,
  ) =>
    request(
      `/workspaces/${id}/branch`,
      workspaceSchema.refine(
        (workspace) =>
          workspace.id !== id && (!input.title || workspace.title === input.title.trim()),
      ),
      'POST',
      input,
      signal,
    ),
  deleteTurn: (workspace: string, runId: string, signal?: AbortSignal) =>
    request(
      `/workspaces/${workspace}/messages/${runId}`,
      z.undefined(),
      'DELETE',
      undefined,
      signal,
    ),
  deleteWorkspace: async (id: string, signal?: AbortSignal) => {
    try {
      await request(`/workspaces/${id}`, z.undefined(), 'DELETE', undefined, signal);
    } catch (error) {
      // Guest routes conceal both absent and foreign resources with 404.
      if (!(error instanceof HttpError && error.status === 404)) throw error;
    }
  },
  submit: (id: string, input: RunRequest) =>
    request(
      `/workspaces/${id}/runs`,
      runSchema.refine(
        (run) =>
          run.workspace_id === id &&
          (input.model_selection?.mode === 'auto'
            ? run.model_route?.automatic === true &&
              run.model_route.task === input.model_selection.task
            : input.model_selection?.mode === 'profile'
              ? run.model_route?.automatic === false &&
                run.model_route.profile_id === input.model_selection.profile_id
              : !run.model_route) &&
          run.workspace_tools === (input.workspace_tools ?? false) &&
          (run.execution_result?.id ?? undefined) === input.execution_result_id &&
          run.search_enabled === (input.search ?? false) &&
          run.import_enabled === (input.allow_url_import ?? false) &&
          (run.image_profile?.id ?? undefined) === input.image_profile_id &&
          (run.image_input?.asset.id ?? undefined) === input.image_input?.asset_id &&
          (run.image_input?.role ?? undefined) === input.image_input?.role &&
          run.image_attachments.map((image) => image.id).join(',') ===
            (input.image_attachment_ids ?? []).join(',') &&
          (run.execution_revision_id ?? undefined) === input.execution_revision_id &&
          run.node_scope ===
            (input.node_scope ?? (input.execution_revision_id ? 'revision' : 'off')) &&
          (!input.input_asset_ids ||
            run.input_assets.map((asset) => asset.id).join(',') ===
              input.input_asset_ids.join(',')),
      ),
      'POST',
      input,
    ),
  cancel: (id: string) => request(`/runs/${id}`, runSchema, 'DELETE'),
  applyProposal: (id: string, signal?: AbortSignal) =>
    request(`/proposals/${id}/apply`, revisionSchema, 'POST', undefined, signal),
  rejectProposal: (id: string, reason: string, signal?: AbortSignal) =>
    request(
      `/proposals/${id}/reject`,
      proposalSchema.refine((proposal) => proposal.id === id && proposal.status === 'rejected'),
      'POST',
      { reason },
      signal,
    ),
};
