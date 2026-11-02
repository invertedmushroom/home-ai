import { z } from 'zod';
import { diffLines } from 'diff';

export function validPath(path: string) {
  return (
    new RegExp('^[A-Za-z0-9._/-]{1,240}$').test(path) &&
    path.split('/').every((part) => {
      const name = part.split('.')[0].toUpperCase();
      return (
        part !== '' &&
        part !== '.' &&
        part !== '..' &&
        !part.endsWith('.') &&
        !/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/.test(name)
      );
    })
  );
}

const pathSchema = z.string().refine(validPath, 'Invalid project path.');
export const assetBindingSchema = z
  .object({
    path: pathSchema,
    asset_id: z.string().uuid(),
  })
  .strict();
export const assetBindingsSchema = z
  .array(assetBindingSchema)
  .max(64)
  .refine(
    (bindings) => new Set(bindings.map((item) => item.path.toLowerCase())).size === bindings.length,
    'Asset binding paths must be unique.',
  );
export const assetChangeSchema = z.discriminatedUnion('op', [
  z.object({ op: z.literal('bind'), path: pathSchema, asset_id: z.string().uuid() }).strict(),
  z.object({ op: z.literal('unbind'), path: pathSchema }).strict(),
]);
export const fileChangeSchema = z.discriminatedUnion('op', [
  z.object({ op: z.literal('write'), path: pathSchema, content: z.string() }).strict(),
  z.object({ op: z.literal('delete'), path: pathSchema }).strict(),
]);
export const fileSchema = z.object({ path: pathSchema, content: z.string() }).strict();
export const filesSchema = z
  .array(fileSchema)
  .max(64)
  .refine((files) => {
    const paths = new Set(files.map((file) => file.path.toLowerCase()));
    return (
      paths.size === files.length &&
      files.reduce(
        (size, file) => size + new TextEncoder().encode(file.path + file.content).length,
        0,
      ) <= 1_048_576 &&
      [...paths].every((path) => {
        const segments = path.split('/');
        return segments
          .slice(0, -1)
          .every((_, index) => !paths.has(segments.slice(0, index + 1).join('/')));
      })
    );
  }, 'Project paths or size are inconsistent.');

export const changesSchema = z
  .array(fileChangeSchema)
  .min(1)
  .max(64)
  .refine(
    (changes) =>
      new Set(changes.map((change) => change.path.toLowerCase())).size === changes.length,
    'Changed paths must be unique.',
  );

export type ProjectFile = z.infer<typeof fileSchema>;
export type FileChange = z.infer<typeof changesSchema>[number];
export type AssetBinding = z.infer<typeof assetBindingSchema>;
export type AssetChange = z.infer<typeof assetChangeSchema>;

export const projectSnapshotSchema = z
  .object({
    files: filesSchema,
    asset_bindings: assetBindingsSchema,
  })
  .strict()
  .refine(({ files, asset_bindings }) => {
    const paths = [...files, ...asset_bindings].map((item) => item.path.toLowerCase());
    const unique = new Set(paths);
    return (
      paths.length <= 64 &&
      unique.size === paths.length &&
      paths.every((path) => {
        const segments = path.split('/');
        return segments
          .slice(0, -1)
          .every((_, index) => !unique.has(segments.slice(0, index + 1).join('/')));
      })
    );
  }, 'Source and asset paths overlap or exceed the project budget.');

export type ManualEditRequest = {
  id: string;
  base_revision: { id: string | null };
  changes: FileChange[];
  asset_changes?: AssetChange[];
};
export type EditorDraft = {
  baseId: string | null;
  baseFiles: ProjectFile[];
  files: ProjectFile[];
  baseBindings?: AssetBinding[];
  assetBindings?: AssetBinding[];
  path: string;
  request?: ManualEditRequest;
};

export function sameAssetChanges(a: AssetChange[] = [], b: AssetChange[] = []) {
  return (
    a.length === b.length &&
    a.every((change, index) => {
      const other = b[index];
      return (
        change.op === other.op &&
        change.path === other.path &&
        (change.op === 'unbind' || (other.op === 'bind' && change.asset_id === other.asset_id))
      );
    })
  );
}

export function applyAssetChanges(base: AssetBinding[], changes: AssetChange[]): AssetBinding[] {
  const bindings = new Map(base.map((item) => [item.path, item.asset_id]));
  const seen = new Set<string>();
  for (const raw of changes) {
    const change = assetChangeSchema.parse(raw);
    if (seen.has(change.path.toLowerCase())) throw new Error('Changed asset paths must be unique.');
    seen.add(change.path.toLowerCase());
    if (change.op === 'unbind') {
      if (!bindings.delete(change.path)) throw new Error('An unbound asset path is missing.');
    } else {
      if (bindings.get(change.path) === change.asset_id)
        throw new Error('Asset binding is unchanged.');
      bindings.set(change.path, change.asset_id);
    }
  }
  return assetBindingsSchema.parse(
    [...bindings]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([path, asset_id]) => ({ path, asset_id })),
  );
}

export function projectAssetChanges(base: AssetBinding[], bindings: AssetBinding[]): AssetChange[] {
  const previous = new Map(base.map((item) => [item.path, item.asset_id]));
  const current = new Set(bindings.map((item) => item.path));
  return [
    ...bindings
      .filter((item) => previous.get(item.path) !== item.asset_id)
      .map((item): AssetChange => ({ op: 'bind', ...item })),
    ...base
      .filter((item) => !current.has(item.path))
      .map((item): AssetChange => ({ op: 'unbind', path: item.path })),
  ].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

export function sameChanges(a: FileChange[], b: FileChange[]) {
  return (
    a.length === b.length &&
    a.every((change, index) => {
      const other = b[index];
      return (
        change.op === other.op &&
        change.path === other.path &&
        (change.op === 'delete' || (other.op === 'write' && change.content === other.content))
      );
    })
  );
}

export function projectChanges(base: ProjectFile[], files: ProjectFile[]): FileChange[] {
  const previous = new Map(base.map((file) => [file.path, file.content]));
  const current = new Set(files.map((file) => file.path));
  return [
    ...files
      .filter((file) => previous.get(file.path) !== file.content)
      .map((file): FileChange => ({ op: 'write', ...file })),
    ...base
      .filter((file) => !current.has(file.path))
      .map((file): FileChange => ({ op: 'delete', path: file.path })),
  ].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

export function applyChanges(base: ProjectFile[], changes: FileChange[]): ProjectFile[] {
  const files = new Map(base.map((file) => [file.path, file.content]));
  for (const change of changes) {
    if (change.op === 'delete') {
      if (!files.delete(change.path)) throw new Error('A deleted file is missing from the base.');
    } else {
      if (files.get(change.path) === change.content)
        throw new Error('A proposed write is unchanged.');
      files.set(change.path, change.content);
    }
  }
  return filesSchema.parse(
    [...files]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([path, content]) => ({ path, content })),
  );
}

export function fileDiff(before: string, after: string) {
  if (
    before.length + after.length > 80_000 ||
    before.split('\n').length + after.split('\n').length > 2000
  )
    return undefined;
  return diffLines(before, after, { timeout: 50, maxEditLength: 1000 });
}
