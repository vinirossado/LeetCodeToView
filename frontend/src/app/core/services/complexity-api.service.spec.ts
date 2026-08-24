import { HttpClientTestingModule, HttpTestingController } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom } from 'rxjs';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { environment } from '../../../environments/environment';
import { ComplexityApiService } from './complexity-api.service';

describe('ComplexityApiService (POST /analysis, real endpoint contract)', () => {
  let service: ComplexityApiService;
  let httpMock: HttpTestingController;

  beforeEach(() => {
    TestBed.configureTestingModule({ imports: [HttpClientTestingModule] });
    service = TestBed.inject(ComplexityApiService);
    httpMock = TestBed.inject(HttpTestingController);
  });

  afterEach(() => {
    httpMock.verify();
  });

  it('POSTs language + code and returns the methods on 200', async () => {
    const promise = firstValueFrom(service.analyze('java', 'int x = 1;'));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/analysis`);
    expect(req.request.method).toBe('POST');
    expect(req.request.body).toEqual({ language: 'java', code: 'int x = 1;' });
    req.flush({
      methods: [{ method_name: 'main', line: 1, time: 'Constant', space: 'Constant', evidence: [] }],
    });

    const outcome = await promise;
    expect(outcome).toEqual({
      kind: 'ok',
      methods: [{ method_name: 'main', line: 1, time: 'Constant', space: 'Constant', evidence: [] }],
    });
  });

  it('maps 501 to the unsupported_language outcome (current real state for C#)', async () => {
    const promise = firstValueFrom(service.analyze('csharp', 'Console.WriteLine(1);'));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/analysis`);
    req.flush(
      { error: 'static analysis not implemented yet for language: csharp' },
      { status: 501, statusText: 'Not Implemented' },
    );

    const outcome = await promise;
    expect(outcome).toEqual({
      kind: 'unsupported_language',
      message: 'static analysis not implemented yet for language: csharp',
    });
  });

  it('maps 422 to the error outcome, carrying the backend message', async () => {
    const promise = firstValueFrom(service.analyze('java', ''));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/analysis`);
    req.flush({ error: 'code is required' }, { status: 422, statusText: 'Unprocessable Entity' });

    const outcome = await promise;
    expect(outcome).toEqual({ kind: 'error', message: 'code is required' });
  });

  it('maps 500 to the error outcome', async () => {
    const promise = firstValueFrom(service.analyze('java', 'int x = 1;'));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/analysis`);
    req.flush({ error: 'unexpected analyzer failure' }, { status: 500, statusText: 'Internal Server Error' });

    const outcome = await promise;
    expect(outcome).toEqual({ kind: 'error', message: 'unexpected analyzer failure' });
  });

  it('falls back to a status-based message when the error body has no "error" field', async () => {
    const promise = firstValueFrom(service.analyze('java', 'int x = 1;'));

    const req = httpMock.expectOne(`${environment.apiBaseUrl}/analysis`);
    req.flush(null, { status: 500, statusText: 'Internal Server Error' });

    const outcome = await promise;
    expect(outcome.kind).toBe('error');
    expect((outcome as { message: string }).message).toContain('500');
  });
});
