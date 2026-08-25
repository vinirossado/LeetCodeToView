// Minimal ambient typing for a Monaco internal module with no shipped
// .d.ts (monaco-editor's public typings only cover the bundled
// `editor.api.d.ts`). See code-editor.component.ts's `TabFocus` import doc
// comment for why this internal module is imported directly.
declare module 'monaco-editor/esm/vs/editor/browser/config/tabFocus.js' {
  export const TabFocus: {
    getTabFocusMode(): boolean;
    setTabFocusMode(tabFocusMode: boolean): void;
  };
}
