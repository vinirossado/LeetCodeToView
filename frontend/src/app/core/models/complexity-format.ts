import type { SpaceComplexity, TimeComplexity } from './complexity.model';

/** Mirrors the Rust `Display` impl for `TimeComplexity` in static-analyzer/src/engine.rs exactly. */
export function formatTimeComplexity(time: TimeComplexity): string {
  if (typeof time === 'string') {
    switch (time) {
      case 'Constant':
        return 'O(1)';
      case 'Logarithmic':
        return 'O(log n)';
      case 'Linear':
        return 'O(n)';
      case 'Linearithmic':
        return 'O(n log n)';
    }
  }
  if ('Polynomial' in time) {
    return `O(n^${time.Polynomial})`;
  }
  return `não foi possível determinar (${time.Unknown})`;
}

/** Mirrors the Rust `Display` impl for `SpaceComplexity` in static-analyzer/src/engine.rs exactly. */
export function formatSpaceComplexity(space: SpaceComplexity): string {
  if (typeof space === 'string') {
    return space === 'Constant' ? 'O(1)' : 'O(n)';
  }
  return `não foi possível determinar (${space.Unknown})`;
}
