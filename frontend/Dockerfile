# Multi-stage: build the Angular production bundle, serve it via nginx.
# Production config uses empty apiBaseUrl/wsBaseUrl (see src/environments/
# environment.ts) — same-origin relative requests, proxied to the API
# container by nginx.conf below. No CORS involved.

FROM node:22-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npx ng build

FROM nginx:1.27-alpine
COPY --from=build /app/dist/frontend/browser /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 80
