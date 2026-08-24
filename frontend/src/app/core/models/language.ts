/** Source languages accepted by `POST /executions` (see spec.md). */
export type Language = 'java' | 'csharp';

export const LANGUAGES: readonly Language[] = ['java', 'csharp'];

/** Human-readable (Portuguese) label for the language selector UI. */
export function languageLabel(language: Language): string {
  switch (language) {
    case 'java':
      return 'Java';
    case 'csharp':
      return 'C#';
  }
}
