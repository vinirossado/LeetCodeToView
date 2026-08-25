import type { Language } from './language';
import type { ExecutionEvent } from './execution-event.model';

export type ExecutionStatus = 'pending' | 'running' | 'completed' | 'failed';

export interface CreateExecutionRequest {
  language: Language;
  code: string;
}

export interface CreateExecutionResponse {
  execution_id: string;
}

export interface TraceResponse {
  execution_id: string;
  status: ExecutionStatus;
  /**
   * The ACTUAL source that was submitted for this execution — added so a
   * reload mid-execution (or opening a shared link) can restore the real
   * code+language into the editor, instead of leaving whatever starter
   * example happened to be showing next to a reconnected trace it could
   * never have produced (see ExecutionSessionService.restoredCode /
   * restoredLanguage, consumed by app.ts).
   */
  language: Language;
  code: string;
  events: ExecutionEvent[];
}

/** Shape of both `POST /executions` 422 responses and any other `{"error": "..."}` body. */
export interface ApiErrorResponse {
  error: string;
}
