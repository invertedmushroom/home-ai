// The iframe supplies the opaque origin. This policy also blocks ordinary resource loads and fetches.
// It is not a CPU, memory or complete network sandbox; native Node jobs use a separate worker.
import projectBootstrap from './preview-project.js?raw';
import type { ProjectFile } from './project';

export function previewDocument(html: string): string {
  const policy =
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'";
  return `<!doctype html><meta http-equiv="Content-Security-Policy" content="${policy}">${html}`;
}

export function projectPreview(
  files: ProjectFile[],
  entry: string,
  resources: Record<string, string> = {},
): string {
  const file = files.find((file) => file.path === entry);
  if (!file || !entry.endsWith('.html')) throw new Error('Select an HTML file to preview.');
  if (files.length === 1 && Object.keys(resources).length === 0)
    return previewDocument(file.content);
  const payload = JSON.stringify({ files, entry, resources, policy: previewDocument('') })
    .replaceAll('<', '\\u003c')
    .replaceAll('\u2028', '\\u2028')
    .replaceAll('\u2029', '\\u2029');
  return previewDocument(
    `<script type="application/json" id="deep-project-data">${payload}</script><script>${projectBootstrap}</script>`,
  );
}
