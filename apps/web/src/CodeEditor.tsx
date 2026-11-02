import { Component, Suspense, lazy } from 'react';
import type { ReactNode } from 'react';

export type CodeEditorProps = {
  identity: string;
  path: string;
  value: string;
  original?: string;
  readOnly?: boolean;
  onChange?: (value: string) => void;
};
const MonacoEditor = lazy(() => import('./MonacoEditor'));

class EditorBoundary extends Component<
  CodeEditorProps & { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div>
        <p className="artifact-note" role="alert">
          Monaco could not load. Your text is available below.
        </p>
        {this.props.original !== undefined && (
          <label>
            Original source
            <textarea
              className="plain-editor"
              aria-label="Original source"
              value={this.props.original}
              readOnly
            />
          </label>
        )}
        <textarea
          className="plain-editor"
          aria-label="File editor"
          value={this.props.value}
          readOnly={this.props.readOnly ?? true}
          onChange={(event) => this.props.onChange?.(event.target.value)}
        />
      </div>
    );
  }
}

export function CodeEditor(props: CodeEditorProps) {
  return (
    <EditorBoundary {...props}>
      <Suspense
        fallback={
          <p className="artifact-note" role="status">
            Loading code editor…
          </p>
        }
      >
        <MonacoEditor {...props} />
      </Suspense>
    </EditorBoundary>
  );
}
