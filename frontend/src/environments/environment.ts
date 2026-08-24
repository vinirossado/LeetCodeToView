// Production environment configuration.
// Empty base URLs mean "same origin as the page" (relative requests) — the
// expected deploy shape is the frontend served behind the same host/reverse
// proxy as the Quarkus API. Override at build time (fileReplacements) for a
// different topology.
export const environment = {
  production: true,
  apiBaseUrl: '',
  wsBaseUrl: '',
};
