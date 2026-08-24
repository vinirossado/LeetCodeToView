import { describe, it, expect } from 'vitest';
import { formatSpaceComplexity, formatTimeComplexity } from './complexity-format';

describe('formatTimeComplexity', () => {
  it('formats Constant as O(1)', () => {
    expect(formatTimeComplexity('Constant')).toBe('O(1)');
  });

  it('formats Logarithmic as O(log n)', () => {
    expect(formatTimeComplexity('Logarithmic')).toBe('O(log n)');
  });

  it('formats Linear as O(n)', () => {
    expect(formatTimeComplexity('Linear')).toBe('O(n)');
  });

  it('formats Linearithmic as O(n log n)', () => {
    expect(formatTimeComplexity('Linearithmic')).toBe('O(n log n)');
  });

  it('formats Polynomial(k) as O(n^k)', () => {
    expect(formatTimeComplexity({ Polynomial: 2 })).toBe('O(n^2)');
    expect(formatTimeComplexity({ Polynomial: 3 })).toBe('O(n^3)');
  });

  it('formats Unknown with the reason, matching the CLI wording', () => {
    expect(formatTimeComplexity({ Unknown: 'saída condicional na linha 5' })).toBe(
      'não foi possível determinar (saída condicional na linha 5)',
    );
  });
});

describe('formatSpaceComplexity', () => {
  it('formats Constant as O(1)', () => {
    expect(formatSpaceComplexity('Constant')).toBe('O(1)');
  });

  it('formats Linear as O(n)', () => {
    expect(formatSpaceComplexity('Linear')).toBe('O(n)');
  });

  it('formats Unknown with the reason', () => {
    expect(formatSpaceComplexity({ Unknown: 'profundidade de pilha não modelada' })).toBe(
      'não foi possível determinar (profundidade de pilha não modelada)',
    );
  });
});
