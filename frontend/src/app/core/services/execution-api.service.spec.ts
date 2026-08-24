import { HttpClientTestingModule, HttpTestingController } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { environment } from '../../../environments/environment';
import { ExecutionApiService } from './execution-api.service';

describe('ExecutionApiService', () => {
  let service: ExecutionApiService;
  let httpMock: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({
      imports: [HttpClientTestingModule],
    });
    service = TestBed.inject(ExecutionApiService);
    httpMock = TestBed.inject(HttpTestingController);
  });

  afterEach(() => {
    httpMock.verify();
  });

  it('POSTs language + code to /executions and returns the execution id', () => {
    let result: { execution_id: string } | undefined;
    service.createExecution({ language: 'java', code: 'int x = 1;' }).subscribe((r) => (result = r));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/executions`);
    expect(req.request.method).toBe('POST');
    expect(req.request.body).toEqual({ language: 'java', code: 'int x = 1;' });
    req.flush({ execution_id: 'b3f1c2a4-6e9d-4a2b-9f3e-1d7c8a0b5f6e' });

    expect(result?.execution_id).toBe('b3f1c2a4-6e9d-4a2b-9f3e-1d7c8a0b5f6e');
  });

  it('GETs the full trace for an execution id', () => {
    let result: unknown;
    service.getTrace('abc-123').subscribe((r) => (result = r));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/executions/abc-123/trace`);
    expect(req.request.method).toBe('GET');
    req.flush({ execution_id: 'abc-123', status: 'completed', events: [] });

    expect(result).toEqual({ execution_id: 'abc-123', status: 'completed', events: [] });
  });
});
