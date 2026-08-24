import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { Injectable, inject } from '@angular/core';
import { type Observable, catchError, map, of } from 'rxjs';
import { environment } from '../../../environments/environment';
import type { AnalysisOutcome, AnalysisResponse } from '../models/analysis.model';
import type { ApiErrorResponse } from '../models/execution.model';
import type { Language } from '../models/language';

function errorMessageOf(err: HttpErrorResponse): string {
  const body = err.error as ApiErrorResponse | null | undefined;
  if (body?.error) return body.error;
  return `${err.status} ${err.statusText}`.trim();
}

/**
 * Client for `POST /analysis` — real endpoint (see tasks.md "Frontend"):
 * request `{language, code}`, synchronous response, no execution_id/trace
 * involved (independent of the sandbox run). Collapses the HTTP status codes
 * into an AnalysisOutcome so callers never touch HttpErrorResponse directly:
 *   200 -> { kind: 'ok', methods }
 *   501 -> { kind: 'unsupported_language', message } (C# has no adapter yet — permanent-for-now, not a bug)
 *   422/500/other -> { kind: 'error', message }
 */
@Injectable({ providedIn: 'root' })
export class ComplexityApiService {
  private readonly http = inject(HttpClient);
  private readonly baseUrl = environment.apiBaseUrl;

  analyze(language: Language, code: string): Observable<AnalysisOutcome> {
    return this.http.post<AnalysisResponse>(`${this.baseUrl}/analysis`, { language, code }).pipe(
      map((res): AnalysisOutcome => ({ kind: 'ok', methods: res.methods })),
      catchError((err: HttpErrorResponse) => {
        const message = errorMessageOf(err);
        const kind = err.status === 501 ? 'unsupported_language' : 'error';
        return of({ kind, message } as AnalysisOutcome);
      }),
    );
  }
}
