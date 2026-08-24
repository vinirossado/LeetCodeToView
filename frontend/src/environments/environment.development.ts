// Development environment configuration — points at the default Quarkus dev
// server (see spec.md). Swapped in for `ng serve` / the "development" build
// configuration via the `fileReplacements` entry in angular.json.
export const environment = {
  production: false,
  apiBaseUrl: 'http://localhost:8080',
  wsBaseUrl: 'ws://localhost:8080',
};
