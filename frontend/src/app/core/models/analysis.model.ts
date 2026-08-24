import type { Language } from './language';
import type { MethodComplexity } from './complexity.model';

/** `POST /analysis` request body — validated against the real API, see tasks.md. */
export interface AnalysisRequest {
  language: Language;
  code: string;
}

/** `POST /analysis` 200 response body. */
export interface AnalysisResponse {
  methods: MethodComplexity[];
}

/**
 * Normalized outcome of calling `POST /analysis`, collapsing the HTTP status
 * codes (200/422/501/500) into one discriminated union so the panel doesn't
 * need to know about HTTP at all.
 *
 * `unsupported_language` is its own case, not folded into `error`: a 501 for
 * C# is the real, permanent-for-now state (only the Java tree-sitter adapter
 * exists), not a transient failure — the UI must not present it as a bug.
 */
export type AnalysisOutcome =
  | { kind: 'ok'; methods: MethodComplexity[] }
  | { kind: 'unsupported_language'; message: string }
  | { kind: 'error'; message: string };
