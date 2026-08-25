// Dev-server-only equivalent of nginx.conf's reverse-proxy rules (see that
// file for the production version). Consumed by `ng serve --proxy-config
// proxy.conf.js`, wired up in docker-compose.dev.yml — the production
// build/Dockerfile never reads this file.
//
// http-proxy-middleware config (same underlying proxy Angular CLI uses).
// "ws: true" on /executions covers GET /executions/:id/events, the one
// WebSocket route under that prefix; harmless for the plain POST/GET
// routes sharing the prefix (matches nginx.conf's Upgrade header handling).
module.exports = {
  "/executions": {
    target: "http://api:8080",
    secure: false,
    changeOrigin: true,
    ws: true,
  },
  "/analysis": {
    target: "http://api:8080",
    secure: false,
    changeOrigin: true,
  },
};
