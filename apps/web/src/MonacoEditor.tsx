import { useEffect, useRef } from 'react';
// Register these before language registration can initialize standalone services.
// Editor contributions reached through Monaco's dependency graph need them too.
import 'monaco-editor/editor/contrib/documentSymbols/browser/outlineModel';
import 'monaco-editor/editor/contrib/codelens/browser/codeLensCache';
import 'monaco-editor/editor/common/services/treeViewsDndService';
import * as monaco from 'monaco-editor/editor';
import 'monaco-editor/editor/browser/coreCommands';
import 'monaco-editor/editor/contrib/clipboard/browser/clipboard';
import 'monaco-editor/editor/contrib/find/browser/findController';
import 'monaco-editor/editor/contrib/contextmenu/browser/contextmenu';
import 'monaco-editor/editor/contrib/tokenization/browser/tokenization';
import 'monaco-editor/editor/contrib/hover/browser/hoverContribution';
import 'monaco-editor/editor/contrib/suggest/browser/suggestController';
import 'monaco-editor/editor/browser/widget/diffEditor/diffEditor.contribution';
import 'monaco-editor/languages/definitions/javascript/register';
import 'monaco-editor/languages/definitions/typescript/register';
import 'monaco-editor/languages/definitions/html/register';
import 'monaco-editor/languages/definitions/css/register';
import * as typescript from 'monaco-editor/languages/features/typescript/register';
import 'monaco-editor/languages/features/html/register';
import 'monaco-editor/languages/features/css/register';
import * as json from 'monaco-editor/languages/features/json/register';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker';
import TypeScriptWorker from 'monaco-editor/language/typescript/ts.worker?worker';
import HtmlWorker from 'monaco-editor/language/html/html.worker?worker';
import CssWorker from 'monaco-editor/language/css/css.worker?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker?worker';
import type { CodeEditorProps } from './CodeEditor';

// Monaco owns its shared language-worker pool; each surface below owns and
// disposes its editor, models, listener and layout observer.
self.MonacoEnvironment = {
  getWorker(_id, label) {
    if (label === 'typescript' || label === 'javascript') return new TypeScriptWorker();
    if (label === 'html') return new HtmlWorker();
    if (label === 'css') return new CssWorker();
    if (label === 'json') return new JsonWorker();
    return new EditorWorker();
  },
};
typescript.javascriptDefaults.setCompilerOptions({
  allowJs: true,
  checkJs: true,
  noEmit: true,
  target: typescript.ScriptTarget.ESNext,
  module: typescript.ModuleKind.ESNext,
});
json.jsonDefaults.setDiagnosticsOptions({ validate: true, enableSchemaRequest: false });

function language(path: string) {
  if (/\.(js|mjs|cjs)$/.test(path)) return 'javascript';
  if (/\.(ts|tsx)$/.test(path)) return 'typescript';
  if (/\.html?$/.test(path)) return 'html';
  if (/\.css$/.test(path)) return 'css';
  if (/\.json$/.test(path)) return 'json';
  return 'plaintext';
}

export default function MonacoEditor({
  identity,
  path,
  value,
  original,
  readOnly = true,
  onChange,
}: CodeEditorProps) {
  const container = useRef<HTMLDivElement>(null);
  const change = useRef(onChange);
  change.current = onChange;
  const initial = useRef({ value, original });
  initial.current = { value, original };
  const model = useRef<monaco.editor.ITextModel | null>(null);
  const instance = useRef<
    monaco.editor.IStandaloneCodeEditor | monaco.editor.IStandaloneDiffEditor | null
  >(null);
  const isDiff = original !== undefined;
  useEffect(() => {
    const host = container.current;
    if (!host) return;
    const models: monaco.editor.ITextModel[] = [];
    let created: NonNullable<typeof instance.current> | undefined;
    let listener: monaco.IDisposable | undefined;
    let resize: ResizeObserver | undefined;
    const dispose = () => {
      resize?.disconnect();
      listener?.dispose();
      created?.dispose();
      for (const item of models.splice(0)) item.dispose();
      instance.current = null;
      model.current = null;
    };
    try {
      const uri = `inmemory://deep/${encodeURIComponent(identity)}/${path}`;
      const modified = monaco.editor.createModel(
        initial.current.value,
        language(path),
        monaco.Uri.parse(uri),
      );
      models.push(modified);
      const before = isDiff
        ? monaco.editor.createModel(
            initial.current.original ?? '',
            language(path),
            monaco.Uri.parse(`inmemory://deep-original/${encodeURIComponent(identity)}/${path}`),
          )
        : null;
      if (before) models.push(before);
      model.current = modified;
      const options: monaco.editor.IStandaloneEditorConstructionOptions = {
        theme: 'vs-dark',
        readOnly,
        ariaLabel: 'File editor',
        fontSize: 12,
        lineNumbers: 'on',
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        wordWrap: 'on',
        padding: { top: 12 },
        automaticLayout: false,
        links: false,
        fixedOverflowWidgets: true,
        accessibilitySupport: 'auto',
        renderValidationDecorations: readOnly ? 'off' : 'on',
      };
      const editor = before
        ? monaco.editor.createDiffEditor(host, {
            ...options,
            renderSideBySide: true,
            useInlineViewWhenSpaceIsLimited: true,
            renderSideBySideInlineBreakpoint: 700,
            maxComputationTime: 50,
            maxFileSize: 1,
            ignoreTrimWhitespace: false,
            originalEditable: false,
          })
        : monaco.editor.create(host, { ...options, model: modified });
      created = editor;
      if (before)
        (editor as monaco.editor.IStandaloneDiffEditor).setModel({ original: before, modified });
      instance.current = editor;
      listener = modified.onDidChangeContent(() =>
        change.current?.(modified.getValue(monaco.editor.EndOfLinePreference.TextDefined, true)),
      );
      const layout = () => {
        if (instance.current !== editor || !host.isConnected) return;
        const { clientWidth: width, clientHeight: height } = host;
        // Diff layout() otherwise measures its own previously sized child.
        // Hidden panes have no usable size; their next observation restores layout.
        if (width > 0 && height > 0) editor.layout({ width, height });
      };
      resize = new ResizeObserver(layout);
      resize.observe(host);
      layout();
    } catch (error) {
      dispose();
      throw error;
    }
    return dispose;
  }, [identity, path, isDiff]);
  useEffect(() => {
    instance.current?.updateOptions({ readOnly });
  }, [readOnly]);
  useEffect(() => {
    const current = model.current;
    if (current && current.getValue(monaco.editor.EndOfLinePreference.TextDefined, true) !== value)
      current.setValue(value);
  }, [value]);
  return (
    <div
      ref={container}
      className="monaco-surface"
      aria-label={isDiff ? 'Code comparison' : 'Code source'}
    />
  );
}
