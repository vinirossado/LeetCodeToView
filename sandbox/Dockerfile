FROM debian:bookworm-slim

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
    ca-certificates \
    openjdk-17-jdk-headless \
    libicu-dev \
    python3 \
    strace \
    && rm -rf /var/lib/apt/lists/*

# .NET SDK via instalador oficial da Microsoft (não depende de pacote apt, igual fizemos com Rust via rustup)
RUN curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --channel 8.0 --install-dir /usr/share/dotnet \
    && rm /tmp/dotnet-install.sh
ENV PATH="/usr/share/dotnet:${PATH}"
ENV DOTNET_ROOT="/usr/share/dotnet"

# netcoredbg: debugger CoreCLR que fala DAP (Debug Adapter Protocol) via stdio — spike da Fase 1 pra instrumentação C#
# release "latest" precisa de glibc 2.38+ (mais nova que a do bookworm, 2.36) — usando release anterior compatível
RUN curl -sSL https://github.com/Samsung/netcoredbg/releases/download/3.0.0-1006/netcoredbg-linux-arm64.tar.gz -o /tmp/netcoredbg.tar.gz \
    && mkdir -p /usr/share/netcoredbg \
    && tar -xzf /tmp/netcoredbg.tar.gz -C /usr/share/netcoredbg \
    && rm /tmp/netcoredbg.tar.gz
ENV PATH="/usr/share/netcoredbg/netcoredbg:${PATH}"

# nsjail não tem pacote no apt — build a partir do source
RUN git clone --depth 1 https://github.com/google/nsjail.git /tmp/nsjail \
    && cd /tmp/nsjail && make \
    && cp nsjail /usr/local/bin/nsjail \
    && cd / && rm -rf /tmp/nsjail

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /app
COPY . .
RUN cargo build --release
RUN javac -encoding UTF-8 -d /app/jdi-out jdi/Debugger.java

ENTRYPOINT ["/app/target/release/sandbox-spike"]
