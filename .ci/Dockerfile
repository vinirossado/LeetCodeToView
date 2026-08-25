# Production image for the API service: Quarkus API + sandbox-runner (Rust/
# nsjail) + static-analyzer, still ONE container — same combined-image
# choice as the root Dockerfile.api, for the same reason (see that file's
# own header comment and tasks.md "Definir contrato/IPC entre API e Sandbox
# Controller"): ProcessSandboxRunner/ProcessStaticAnalyzer shell out to these
# binaries as LOCAL subprocesses (ProcessBuilder, absolute paths on the same
# filesystem), not over any network/IPC protocol, so the binaries have to be
# on this container's own filesystem. This is NOT the target production
# shape (a Sandbox Controller running untrusted code shouldn't share a
# container — or its privileges — with the public-facing API); revisit when
# a real IPC contract exists. Documented here again, not newly introduced.
#
# What THIS file changes vs. Dockerfile.api (production hygiene pass):
#   1. Build tools (maven, rust/cargo, build-essential, git, flex/bison,
#      gcc, the -dev headers for nsjail's deps, python3) live ONLY in the
#      build stages below and are never present in the final image — unlike
#      Dockerfile.api, whose runtime stage is `FROM toolchain` and therefore
#      ships the entire build toolchain in production for no reason.
#   2. A HEALTHCHECK against the new GET /health endpoint (api/.../web/
#      HealthResource.java) — Quarkus had no health endpoint before this;
#      added as a minimal liveness probe only (see that file's comment).
#   3. Only the runtime shared libraries nsjail actually links against
#      (confirmed via `ldd` against the binary built below, not guessed)
#      are installed in the final stage, instead of the full -dev/build
#      package set.
#
# Still true here, unchanged from Dockerfile.api/docker-compose.yml: this
# container needs nsjail's namespace/cgroup privileges to sandbox anything.
# In `docker run`/Compose that's `privileged: true` + `cgroup: host`. Docker
# SWARM stack files do NOT support `privileged: true` at all (Swarm
# intentionally dropped that field) — see .ci/stack.yml's comment header for
# how this is being handled (spoiler: it isn't, cleanly, yet — flagged as an
# open question in tasks.md rather than papered over).

# ─── Shared build toolchain (build-only, never shipped) ──────────────────
FROM debian:bookworm-slim AS toolchain

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    flex bison \
    pkg-config \
    libprotobuf-dev protobuf-compiler \
    libnl-route-3-dev \
    libcap-dev \
    libseccomp-dev \
    git \
    curl \
    gnupg \
    ca-certificates \
    maven \
    unzip \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

# JDK 25, used for BOTH building/running the Quarkus API AND (via
# update-alternatives, `/usr/bin/java`/`/usr/bin/javac`) running sandboxed
# user code — see Dockerfile.api's identical, longer comment for why this
# is now a single JDK instead of two: an `openjdk-17-jdk-headless` install
# used to sit here too, on the mistaken assumption the sandboxed path
# needed to "stay JDK 17", but `update-alternatives`'s default
# highest-version-wins behavior meant `/usr/bin/java` silently resolved to
# this JDK 25 install regardless — found empirically, not assumed. Removed
# rather than fixed to actually enforce JDK 17, since everything already
# works correctly under JDK 25. Install dir is architecture-suffixed
# (temurin-25-jdk-amd64 vs -arm64); symlinked to a stable path instead of
# hardcoding an arch (found empirically building on an arm64 host).
RUN curl -fsSL https://packages.adoptium.net/artifactory/api/gpg/key/public | gpg --dearmor -o /usr/share/keyrings/adoptium.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/adoptium.gpg] https://packages.adoptium.net/artifactory/deb bookworm main" > /etc/apt/sources.list.d/adoptium.list \
    && apt-get update && apt-get install -y --no-install-recommends temurin-25-jdk \
    && rm -rf /var/lib/apt/lists/* \
    && ln -s /usr/lib/jvm/temurin-25-jdk-* /usr/lib/jvm/jdk-25
ENV JDK25_HOME="/usr/lib/jvm/jdk-25"

# nsjail has no apt package — build from source. Only the resulting binary
# (plus its runtime .so deps, installed separately below) is copied into
# the final image; the -dev headers/build-essential used here are not.
RUN git clone --depth 1 https://github.com/google/nsjail.git /tmp/nsjail \
    && cd /tmp/nsjail && make \
    && cp nsjail /usr/local/bin/nsjail \
    && cd / && rm -rf /tmp/nsjail

# ─── Build sandbox-runner + jdi Debugger ──────────────────────────────────
FROM toolchain AS sandbox-build
WORKDIR /build/sandbox
COPY sandbox/ .
RUN cargo build --release
RUN javac -encoding UTF-8 -d /build/jdi-out jdi/Debugger.java

# ─── CDS (Class Data Sharing) archive for the JDI driver ──────────────────
# Same mechanism/rationale as Dockerfile.api's identical (much more
# thoroughly commented) step — jar up the driver's fixed classpath
# (dynamic CDS archiving, -XX:ArchiveClassesAtExit, refuses to dump a
# directory classpath entry, confirmed empirically) and train a dynamic
# archive against it via a real end-to-end run of test-snippets/
# Trivial.java through the actual Debugger driver. Generated at the SAME
# /app/... path the final runtime stage below copies it to (CDS validates
# the app classpath by exact path string). This RUN sits AFTER the
# `javac ... Debugger.java` layer above, so normal Docker layer caching
# already invalidates/regenerates it whenever Debugger.java's compiled
# output changes — no --no-cache needed. `test -s`: fail the build loudly
# if the archive didn't come out, same "never assume, validate" discipline
# as the rest of this project — safe to be this strict because java.rs
# ships -Xshare:auto (not `on`), so a missing/bad archive at runtime would
# only silently disable this optimization, not crash executions.
#
# CAVEAT specific to THIS file (not Dockerfile.api): unlike Dockerfile.api,
# whose final runtime stage is `FROM toolchain` (the exact same image this
# archive is trained in), this file's `runtime` stage below is a FRESH
# `debian:bookworm-slim` base with its OWN separate `apt-get install
# temurin-25-jdk` — so in principle a different JDK point-release resolved
# by that separate install could shift the archived boot-classpath's
# absolute path enough to fail CDS's own validation. Not a correctness risk
# either way: -Xshare:auto (see java.rs) falls back to the JDK's own
# default archive silently on any such mismatch instead of crashing — this
# is flagged here as a possible (not confirmed) source of a smaller-than-
# expected win on THIS image specifically, not as a bug.
RUN mkdir -p /app /build/cds-train && \
    jar cf /app/jdi-driver.jar -C /build/jdi-out . && \
    cp test-snippets/Trivial.java /build/cds-train/ && \
    javac -encoding UTF-8 -g -d /build/cds-train /build/cds-train/Trivial.java && \
    java -Xlog:cds -XX:ArchiveClassesAtExit=/app/jdi-driver.jsa \
        -cp /app/jdi-driver.jar Debugger Trivial \
        "-cp /build/cds-train -Xmx256m -XX:MaxMetaspaceSize=64m -XX:MaxDirectMemorySize=256m" && \
    test -s /app/jdi-driver.jsa

# ─── Build static-analyzer ────────────────────────────────────────────────
FROM toolchain AS analyzer-build
WORKDIR /build/static-analyzer
COPY static-analyzer/ .
RUN cargo build --release

# ─── Build the Quarkus API ────────────────────────────────────────────────
FROM toolchain AS api-build
WORKDIR /build/api
COPY api/pom.xml .
COPY api/.mvn .mvn
COPY api/mvnw .
RUN JAVA_HOME=$JDK25_HOME ./mvnw -q -B dependency:go-offline || true
COPY api/src src
RUN JAVA_HOME=$JDK25_HOME ./mvnw -q -B package -DskipTests

# ─── Runtime ───────────────────────────────────────────────────────────────
# Fresh minimal base — deliberately NOT `FROM toolchain`, so none of the
# build-only tooling above (maven, rust, build-essential, git, gcc,
# flex/bison, -dev headers) ends up in the image that actually runs in
# production.
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    gnupg \
    # libicu72: the .NET SDK's own CLI/MSBuild (not just user C# code)
    # needs ICU for globalization unless invariant mode is forced — without
    # it `dotnet build` can fail outright with "Couldn't find a valid ICU
    # package installed on the system". Kept as a real runtime dependency
    # (matches sandbox/Dockerfile's original libicu-dev, trimmed to the
    # non-dev runtime package since nothing here compiles against it).
    libicu72 \
    # runtime (non -dev) shared libs nsjail links against — verified via
    # `ldd /usr/local/bin/nsjail` against the binary built above, not
    # guessed from the -dev package names used to build it.
    libprotobuf32 \
    libnl-3-200 \
    libnl-route-3-200 \
    libcap2 \
    libseccomp2 \
    # uidmap: provides newuidmap/newgidmap — see Dockerfile.api's identical
    # comment for why nsjail's --uid_mapping/--gid_mapping (not the simpler
    # --user/--group) is what actually achieves non-root sandboxed
    # execution, and why this package is required for that.
    uidmap \
    && rm -rf /var/lib/apt/lists/*

# See Dockerfile.api's identical comment: newuidmap/newgidmap require this
# delegation even for real root.
RUN echo "root:65534:1" > /etc/subuid && echo "root:65534:1" > /etc/subgid

# JDK 25 — runs the Quarkus API jar itself AND (via update-alternatives,
# `/usr/bin/java`/`javac`) sandboxed user code; see the toolchain stage's
# identical comment above for why this is one JDK, not the originally
# planned two. Same package/symlink approach as the build stage — this is
# a separate apt install because this is a separate, non-toolchain base
# image.
RUN curl -fsSL https://packages.adoptium.net/artifactory/api/gpg/key/public | gpg --dearmor -o /usr/share/keyrings/adoptium.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/adoptium.gpg] https://packages.adoptium.net/artifactory/deb bookworm main" > /etc/apt/sources.list.d/adoptium.list \
    && apt-get update && apt-get install -y --no-install-recommends temurin-25-jdk \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get purge -y gnupg && apt-get autoremove -y \
    && ln -s /usr/lib/jvm/temurin-25-jdk-* /usr/lib/jvm/jdk-25

# .NET SDK — needed at RUNTIME, not just at build time: ProcessSandboxRunner
# compiles user-submitted C# with `dotnet build` on every execution request
# (see api/.../sandbox/ProcessSandboxRunner.java#compileCsharp), so the full
# SDK (not just a runtime) must be present in the running container.
RUN curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --channel 8.0 --install-dir /usr/share/dotnet \
    && rm /tmp/dotnet-install.sh
ENV PATH="/usr/share/dotnet:${PATH}"
ENV DOTNET_ROOT="/usr/share/dotnet"

# netcoredbg provides libdbgshim.so, dlopen'd directly by csharp.rs's
# ICorDebug worker (LIBDBGSHIM_PATH) — netcoredbg's own DAP debugger binary
# is unused in production (abandoned for direct ICorDebug interop, see
# spec.md "Estratégia C#"), only this one shared library matters. Missing
# it fails every C# execution with "cannot open shared object file" (found
# empirically against Dockerfile.api earlier this session — same fix here).
# linux-arm64 build, matching Dockerfile.api's existing pin; not verified
# on amd64 hosts.
RUN curl -sSL https://github.com/Samsung/netcoredbg/releases/download/3.0.0-1006/netcoredbg-linux-arm64.tar.gz -o /tmp/netcoredbg.tar.gz \
    && mkdir -p /usr/share/netcoredbg \
    && tar -xzf /tmp/netcoredbg.tar.gz -C /usr/share/netcoredbg \
    && rm /tmp/netcoredbg.tar.gz

COPY --from=sandbox-build /usr/local/bin/nsjail /usr/local/bin/nsjail
COPY --from=sandbox-build /build/sandbox/target/release/sandbox-runner /app/sandbox-runner
# jdi-driver.jar/.jsa (NOT the old exploded jdi-out/ dir) — see the
# sandbox-build stage's CDS archive-generation comment above and
# Dockerfile.api's identical COPY comment. Same /app/... path used both at
# archive-generation time and here (required for CDS's app-classpath
# validation to pass).
COPY --from=sandbox-build /app/jdi-driver.jar /app/jdi-driver.jar
COPY --from=sandbox-build /app/jdi-driver.jsa /app/jdi-driver.jsa
COPY --from=analyzer-build /build/static-analyzer/target/release/static-analyzer /app/static-analyzer
COPY --from=api-build /build/api/target/quarkus-app /app/quarkus-app

# Minimal, isolated rootfs per sandboxed language -- same as Dockerfile.api,
# see sandbox/build-minimal-rootfs.sh's header comment and tasks.md
# "Filesystem temporário/efêmero" for the full rationale. Must run AFTER the
# COPYs above (reads the already-built /app/jdi-driver.jar, /app/jdi-driver.jsa
# and /app/sandbox-runner).
COPY sandbox/build-minimal-rootfs.sh /tmp/build-minimal-rootfs.sh
RUN bash /tmp/build-minimal-rootfs.sh /opt/sandbox-rootfs && rm /tmp/build-minimal-rootfs.sh

ENV SANDBOX_RUNNER_BIN="/app/sandbox-runner"
ENV STATIC_ANALYZER_BIN="/app/static-analyzer"

EXPOSE 8080

# Liveness only (see HealthResource.java) — does not verify sandbox-runner/
# static-analyzer are reachable, only that the Quarkus HTTP layer answers.
HEALTHCHECK --interval=10s --timeout=3s --start-period=60s --retries=3 \
    CMD curl -f http://127.0.0.1:8080/health || exit 1

CMD ["/usr/lib/jvm/jdk-25/bin/java", "-Dsandbox.runner.binary-path=/app/sandbox-runner", \
     "-Dstatic-analyzer.binary-path=/app/static-analyzer", \
     "-jar", "/app/quarkus-app/quarkus-run.jar"]
