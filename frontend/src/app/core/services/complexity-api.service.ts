import { Injectable } from '@angular/core';
import type { Language } from '../models/language';
import type { MethodComplexity } from '../models/complexity.model';

/**
 * TODO(backend gap — see tasks.md "Frontend"): there is currently no live
 * HTTP endpoint for static complexity analysis. `static-analyzer/` is only a
 * CLI (`cargo run -- file.java [--json]`); it is not wired into the API. This
 * service stands in for that endpoint with a tiny heuristic mock so the
 * complexity panel has something to render and can be swapped for a real
 * `ExecutionApiService`-style HTTP call later (same method signature) once
 * `POST /analysis` (or similar) exists — presumably following the same
 * subprocess pattern as `ProcessSandboxRunner`
 * (api/src/main/java/com/code2complexity/api/sandbox/ProcessSandboxRunner.java).
 *
 * Returns `null` to represent "não determinado" (analysis unavailable/not
 * recognized), matching the analyzer's own honest-uncertainty philosophy
 * (spec.md "Limitação conhecida: inferir complexidade... é fundamentalmente
 * heurístico") rather than ever fabricating a confident-looking Big-O guess.
 */
@Injectable({ providedIn: 'root' })
export class ComplexityApiService {
  async analyze(_language: Language, code: string): Promise<MethodComplexity[] | null> {
    const nestedLoops = (code.match(/for\s*\(/g) ?? []).length >= 2;
    const singleLoop = (code.match(/for\s*\(/g) ?? []).length === 1;

    if (nestedLoops) {
      return [
        {
          method_name: 'main',
          line: 1,
          time: { Polynomial: 2 },
          space: 'Constant',
          evidence: ['[mock] detectados 2 laços aninhados no código enviado'],
        },
      ];
    }

    if (singleLoop) {
      return [
        {
          method_name: 'main',
          line: 1,
          time: 'Linear',
          space: 'Constant',
          evidence: ['[mock] detectado 1 laço no código enviado'],
        },
      ];
    }

    return null;
  }
}
