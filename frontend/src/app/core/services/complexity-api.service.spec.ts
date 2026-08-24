import { describe, expect, it } from 'vitest';
import { ComplexityApiService } from './complexity-api.service';

describe('ComplexityApiService (mocked — no live endpoint yet, see tasks.md)', () => {
  it('resolves to null (não determinado) for code it does not recognize', async () => {
    const service = new ComplexityApiService();
    const result = await service.analyze('java', 'qualquer coisa não reconhecida pelo mock');
    expect(result).toBeNull();
  });

  it('resolves to a canned O(n^2) result for the nested-loop mock trigger', async () => {
    const service = new ComplexityApiService();
    const result = await service.analyze('java', 'for (int i = 0; i < n; i++) { for (int j = 0; j < n; j++) {} }');
    expect(result).not.toBeNull();
    expect(result?.[0].time).toEqual({ Polynomial: 2 });
  });

  it('resolves to a canned O(n) result for the single-loop mock trigger', async () => {
    const service = new ComplexityApiService();
    const result = await service.analyze('java', 'for (int i = 0; i < n; i++) {}');
    expect(result).not.toBeNull();
    expect(result?.[0].time).toBe('Linear');
  });
});
