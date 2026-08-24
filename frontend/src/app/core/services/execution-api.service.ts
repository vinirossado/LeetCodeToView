import { HttpClient } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import type { Observable } from 'rxjs';
import { environment } from '../../../environments/environment';
import type {
  CreateExecutionRequest,
  CreateExecutionResponse,
  TraceResponse,
} from '../models/execution.model';

/**
 * Thin REST client for the two HTTP endpoints in the backend contract
 * (spec.md): creating an execution and fetching its full trace. The
 * WebSocket endpoint is handled separately by ExecutionSocketService.
 */
@Injectable({ providedIn: 'root' })
export class ExecutionApiService {
  private readonly http = inject(HttpClient);
  private readonly baseUrl = environment.apiBaseUrl;

  createExecution(request: CreateExecutionRequest): Observable<CreateExecutionResponse> {
    return this.http.post<CreateExecutionResponse>(`${this.baseUrl}/executions`, request);
  }

  getTrace(executionId: string): Observable<TraceResponse> {
    return this.http.get<TraceResponse>(`${this.baseUrl}/executions/${executionId}/trace`);
  }
}
