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
  events: ExecutionEvent[];
}

/** Shape of both `POST /executions` 422 responses and any other `{"error": "..."}` body. */
export interface ApiErrorResponse {
  error: string;
}
