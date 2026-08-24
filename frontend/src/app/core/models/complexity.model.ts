// Mirrors the JSON shape actually produced by `static-analyzer --json`
// (verified by running the CLI against static-analyzer/test-snippets/*.java —
// see engine.rs `TimeComplexity`/`SpaceComplexity` enums with #[derive(Serialize)],
// which serde encodes as: unit variants -> plain string, tuple variants -> a
// single-key object, e.g. {"Polynomial": 2} or {"Unknown": "reason text"}).
// There is no live HTTP endpoint for this yet — see complexity-api.service.ts.

export type TimeComplexity =
  | 'Constant'
  | 'Logarithmic'
  | 'Linear'
  | 'Linearithmic'
  | { Polynomial: number }
  | { Unknown: string };

export type SpaceComplexity = 'Constant' | 'Linear' | { Unknown: string };

export interface MethodComplexity {
  method_name: string;
  line: number;
  time: TimeComplexity;
  space: SpaceComplexity;
  evidence: string[];
}
