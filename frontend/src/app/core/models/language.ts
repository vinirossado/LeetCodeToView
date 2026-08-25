/** Source languages accepted by `POST /executions` (see spec.md). */
export type Language = 'java' | 'csharp' | 'ruby';

export const LANGUAGES: readonly Language[] = ['java', 'csharp', 'ruby'];

/** Human-readable (Portuguese) label for the language selector UI. */
export function languageLabel(language: Language): string {
  switch (language) {
    case 'java':
      return 'Java';
    case 'csharp':
      return 'C#';
    case 'ruby':
      return 'Ruby';
  }
}
